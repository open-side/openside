import { Copy, ExternalLink } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { ApiError, MESSAGES, type Client } from "../api/client";
import type { OAuthMethod, OAuthProviderInfo, OAuthTransaction, OAuthTransactionStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { ErrorNote } from "../pages/blocks";
import { formatRelative } from "../pages/format";
import { Button, copyText, Dialog, DialogActions, IconButton, ICON_STROKE, Spinner, useToast } from "../ui";
import { LabelField, SECRET_INPUT } from "./AccountDialogs";

/** How often a running sign-in is re-read. */
export const OAUTH_POLL_MS = 1500;

export const TERMINAL_STATUS: Record<OAuthTransactionStatus, boolean> = {
  starting: false,
  awaiting_browser: false,
  awaiting_code: false,
  exchanging: false,
  ready: true,
  failed: true,
  cancelled: true,
  expired: true,
};

/** Why a sign-in stopped, per transaction reason code. */
const REASONS: Record<string, string> = {
  oauth_callback_busy: "Another sign-in is using the sign-in port on the computer running openside. Finish or cancel it, then try again.",
  oauth_invalid_grant: "The provider did not accept this sign-in. Try again.",
  oauth_failed: "The sign-in did not finish. Try again.",
  oauth_flow_unsupported: "This sign-in method is not available for this provider.",
  oauth_helper_unavailable: "openside could not start the sign-in on the computer running it. Run openside doctor there, then try again.",
  oauth_cancelled: "The sign-in was cancelled.",
  oauth_expired: "The sign-in took too long and expired. Try again.",
  oauth_account_removed: "This account was removed before the sign-in finished.",
};

const METHOD_DETAIL: Record<OAuthMethod, string> = {
  browser: "Sign in on the provider's page",
  device: "Enter a code on the provider's page",
};

function reasonCopy(tx: OAuthTransaction): string {
  const reason = tx.reason ?? "";
  return REASONS[reason] ?? MESSAGES[reason] ?? (tx.status === "expired" ? REASONS.oauth_expired! : REASONS.oauth_failed!);
}

/**
 * The browser is not on the computer running openside, so a sign-in page that ends on that
 * computer's loopback address cannot load here.
 */
function onOtherDevice(client: Client): boolean {
  return client.relay || !["localhost", "127.0.0.1", "[::1]", "::1"].includes(window.location.hostname);
}

/**
 * One sign-in transaction at a time: starts it, polls it while it runs and drops answers that
 * belong to a transaction that is no longer current or that were overtaken by a later answer.
 */
interface SignIn {
  tx: OAuthTransaction | null;
  /** A start or paste the daemon refused; transaction failures are in `tx.reason`. */
  error: string | null;
  busy: boolean;
  start: (begin: () => Promise<OAuthTransaction>) => Promise<void>;
  cancel: () => void;
  submit: (code: string) => Promise<boolean>;
}

function useSignIn(onReady: (tx: OAuthTransaction) => void): SignIn {
  const client = useClient();
  const [tx, setTx] = useState<OAuthTransaction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const current = useRef<string | null>(null);
  const live = useRef(false);
  // Every request takes a ticket; an answer older than the last applied one is stale.
  const issued = useRef(0);
  const applied = useRef(0);
  // A start answered after the person cancelled or retried is cancelled on the daemon too.
  const attempt = useRef(0);
  const readyFor = useRef<string | null>(null);
  const ready = useRef(onReady);
  useEffect(() => {
    ready.current = onReady;
  });

  const apply = useCallback((next: OAuthTransaction, ticket: number) => {
    if (next.transaction_id !== current.current || ticket < applied.current) return;
    applied.current = ticket;
    live.current = !TERMINAL_STATUS[next.status];
    setTx(next);
    if (next.status === "ready" && readyFor.current !== next.transaction_id) {
      readyFor.current = next.transaction_id;
      ready.current(next);
    }
  }, []);

  const id = tx?.transaction_id;
  const running = tx ? !TERMINAL_STATUS[tx.status] : false;
  useEffect(() => {
    if (!id || !running) return;
    const timer = window.setInterval(() => {
      const ticket = ++issued.current;
      client.oauthTransaction(id).then(
        (next) => apply(next, ticket),
        (cause) => {
          if (!(cause instanceof ApiError && cause.code === "oauth_transaction_unknown") || id !== current.current || ticket < applied.current) return;
          applied.current = ticket;
          live.current = false;
          setTx((previous) => (previous && previous.transaction_id === id ? { ...previous, status: "failed", reason: cause.code, authorization: null, manual_input: false } : previous));
        },
      );
    }, OAUTH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [client, id, running, apply]);

  const cancel = useCallback(() => {
    attempt.current++;
    const previous = current.current;
    if (previous && live.current) void client.cancelOAuthTransaction(previous).catch(() => undefined);
    current.current = null;
    live.current = false;
    setTx(null);
    setError(null);
    setBusy(false);
  }, [client]);

  const start = useCallback(
    async (begin: () => Promise<OAuthTransaction>) => {
      const mine = ++attempt.current;
      current.current = null;
      live.current = false;
      setTx(null);
      setError(null);
      setBusy(true);
      try {
        const ticket = ++issued.current;
        const next = await begin();
        if (mine !== attempt.current) {
          void client.cancelOAuthTransaction(next.transaction_id).catch(() => undefined);
          return;
        }
        current.current = next.transaction_id;
        apply(next, ticket);
      } catch (cause) {
        if (mine === attempt.current) setError(errorMessage(cause));
      } finally {
        if (mine === attempt.current) setBusy(false);
      }
    },
    [client, apply],
  );

  const submit = useCallback(
    async (code: string): Promise<boolean> => {
      const target = current.current;
      if (!target) return false;
      setError(null);
      try {
        const ticket = ++issued.current;
        apply(await client.submitOAuthInput(target, code), ticket);
        return true;
      } catch (cause) {
        setError(errorMessage(cause));
        return false;
      }
    },
    [client, apply],
  );

  return { tx, error, busy, start, cancel, submit };
}

function CopyButton({ value, label, done }: { value: string; label: string; done: string }) {
  const toast = useToast();
  return (
    <IconButton
      label={label}
      size="small"
      onClick={() => void copyText(value).then((copied) => toast.notify(copied ? done : "Could not copy. Select the text and copy it.", copied ? undefined : "error"))}
    >
      <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </IconButton>
  );
}

/** The pasted code or final page address; cleared as soon as it is sent. */
function ManualInput({ onSubmit }: { onSubmit: (code: string) => Promise<boolean> }) {
  const [value, setValue] = useState("");
  const [sending, setSending] = useState(false);
  async function submit(event: FormEvent) {
    event.preventDefault();
    const code = value.trim();
    if (!code) return;
    setSending(true);
    setValue("");
    await onSubmit(code);
    setSending(false);
  }
  return (
    <form className="oauth-manual" onSubmit={submit} aria-label="Paste the sign-in code">
      <label className="field">
        <span>Paste the code or the final page address</span>
        <input value={value} onChange={(event) => setValue(event.target.value)} maxLength={4096} {...SECRET_INPUT} data-1p-ignore data-lpignore="true" aria-describedby="oauth-manual-help" />
        <small id="oauth-manual-help" className="field-help">
          Only paste the code or the address the sign-in page ends on. Never paste a password or an access token.
        </small>
      </label>
      <Button type="submit" size="compact" disabled={sending || !value.trim()}>
        {sending ? "Sending…" : "Send"}
      </Button>
    </form>
  );
}

/** Where a running sign-in stands, and what the person does next. */
function SignInSteps({ flow, onRetry, onCancel, onDone }: { flow: SignIn; onRetry: () => void; onCancel: () => void; onDone: () => void }) {
  const client = useClient();
  const { tx, error, busy } = flow;

  if (!tx) {
    return (
      <div className="dialog-form">
        {error ? <ErrorNote>{error}</ErrorNote> : <Waiting text="Starting sign-in…" />}
        <DialogActions>
          <Button onClick={onCancel}>{error ? "Close" : "Cancel"}</Button>
          {error ? (
            <Button variant="primary" disabled={busy} onClick={onRetry}>
              Try again
            </Button>
          ) : null}
        </DialogActions>
      </div>
    );
  }

  if (tx.status === "ready") {
    return (
      <div className="dialog-form">
        <p className="signin-done" role="status">
          Connected. {tx.account?.label ?? "This account"} is ready to use.
        </p>
        <DialogActions>
          <Button variant="primary" onClick={onDone}>
            Done
          </Button>
        </DialogActions>
      </div>
    );
  }

  if (TERMINAL_STATUS[tx.status]) {
    return (
      <div className="dialog-form">
        <ErrorNote>{reasonCopy(tx)}</ErrorNote>
        <DialogActions>
          <Button onClick={onDone}>Close</Button>
          <Button variant="primary" disabled={busy} onClick={onRetry}>
            Try again
          </Button>
        </DialogActions>
      </div>
    );
  }

  const authorization = tx.authorization;
  const device = tx.method === "device";
  return (
    <div className="dialog-form">
      {tx.status === "starting" || !authorization ? <Waiting text="Starting sign-in…" /> : null}
      {tx.status === "exchanging" ? <Waiting text="Finishing sign-in…" /> : null}
      {authorization && (tx.status === "awaiting_browser" || tx.status === "awaiting_code") ? (
        <>
          <p className="settings-text">
            {device ? "Open the sign-in page and enter this code there." : "Open the sign-in page and finish signing in there. This window notices when you are done."}
          </p>
          {device && authorization.user_code ? (
            <span className="login-command login-command--block oauth-user-code">
              <code aria-label="Sign-in code">{authorization.user_code}</code>
              <CopyButton value={authorization.user_code} label="Copy sign-in code" done="Code copied." />
            </span>
          ) : null}
          {!device && authorization.instructions ? <p className="settings-text">{authorization.instructions}</p> : null}
          <span className="oauth-open">
            <a className="ui-button ui-button--primary ui-button--compact" href={authorization.url} target="_blank" rel="noopener noreferrer">
              <ExternalLink size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Open sign-in page
            </a>
            <CopyButton value={authorization.url} label="Copy sign-in link" done="Link copied." />
          </span>
          {!device && onOtherDevice(client) ? (
            <p className="field-help oauth-note" role="note">
              The sign-in page may end on an address on the computer running openside, which this device cannot open. If it does, copy that page's address and paste it here when asked.
            </p>
          ) : null}
          {tx.manual_input ? <ManualInput onSubmit={flow.submit} /> : null}
          <Waiting text={`Waiting for the sign-in… Expires ${formatRelative(tx.expires_at_ms)}.`} />
        </>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
      </DialogActions>
    </div>
  );
}

function Waiting({ text }: { text: string }) {
  return (
    <p className="settings-text signin-wait oauth-wait" role="status">
      <Spinner size={14} />
      {text}
    </p>
  );
}

interface Choice {
  info: OAuthProviderInfo;
  method: OAuthMethod;
}

const choiceKey = (choice: Choice) => `${choice.info.provider}:${choice.method}`;

/**
 * Connects an account by signing in to the provider: pick a provider and method, name the
 * account, then follow the sign-in. The account exists only once the sign-in is ready.
 */
export function ConnectAccountDialog({
  open,
  isFirst,
  onClose,
  onReady,
}: {
  open: boolean;
  /** No account serves this provider yet, so the new one always joins its rotation. */
  isFirst: (provider: string) => boolean;
  onClose: () => void;
  onReady: () => void;
}) {
  const client = useClient();
  const providers = useResource(useCallback(() => client.oauthProviders(), [client]));
  const [picked, setPicked] = useState<string>("");
  const [label, setLabel] = useState("");
  const [named, setNamed] = useState(false);
  const [join, setJoin] = useState(true);
  const [started, setStarted] = useState(false);
  const flow = useSignIn(onReady);
  const choices: Choice[] = (providers.data?.providers ?? []).flatMap((info) => info.methods.map((method) => ({ info, method })));
  const choice = choices.find((candidate) => choiceKey(candidate) === picked);
  const first = choice ? isFirst(choice.info.provider) : false;

  function close() {
    flow.cancel();
    setPicked("");
    setLabel("");
    setNamed(false);
    setJoin(true);
    setStarted(false);
    onClose();
  }

  function begin() {
    if (!choice) return;
    const { info, method } = choice;
    const request = { label: label.trim(), join_pool: first || join, ...(info.methods.length > 1 ? { method } : {}) };
    void flow.start(() => client.createOAuthAccount(info.provider, request));
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    setStarted(true);
    begin();
  }

  function pick(next: Choice) {
    setPicked(choiceKey(next));
    if (!named) setLabel(next.info.name);
  }

  return (
    <Dialog
      open={open}
      onClose={close}
      title={choice && started ? `Connect ${choice.info.name}` : "Connect an account"}
      description={started ? undefined : "Sign in with a subscription you already have. The sign-in is kept encrypted in the vault on the computer running openside."}
    >
      {started ? (
        <SignInSteps flow={flow} onRetry={begin} onCancel={close} onDone={close} />
      ) : (
        <form className="dialog-form" onSubmit={submit} aria-label="Connect an account">
          {providers.error ? <ErrorNote>{providers.error}</ErrorNote> : null}
          {providers.data ? (
            <fieldset className="oauth-choices">
              <legend>Provider</legend>
              {choices.map((candidate) => (
                <label key={choiceKey(candidate)} className="oauth-choice">
                  <input type="radio" name="oauth-provider" value={choiceKey(candidate)} checked={picked === choiceKey(candidate)} onChange={() => pick(candidate)} />
                  <span className="oauth-choice-copy">
                    <strong>{candidate.info.name}</strong>
                    <small>{METHOD_DETAIL[candidate.method]}</small>
                  </span>
                </label>
              ))}
            </fieldset>
          ) : providers.error ? null : (
            <Waiting text="Loading providers…" />
          )}
          <LabelField
            value={label}
            onChange={(next) => {
              setNamed(true);
              setLabel(next);
            }}
          />
          {choice && !first ? (
            <label className="check-row">
              <input type="checkbox" checked={join} onChange={(event) => setJoin(event.target.checked)} />
              <span>Use it in the rotation right away</span>
            </label>
          ) : null}
          <DialogActions>
            <Button onClick={close}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!choice || !label.trim()}>
              Continue
            </Button>
          </DialogActions>
        </form>
      )}
    </Dialog>
  );
}

/** Signs an existing account in again; its name, reserve and rotation place are kept. */
export function ReloginDialog({
  open,
  account,
  onClose,
  onReady,
}: {
  open: boolean;
  account: { provider: string; account_id: string; label: string };
  onClose: () => void;
  onReady: () => void;
}) {
  const client = useClient();
  const flow = useSignIn(onReady);
  const { start, cancel } = flow;
  const { provider, account_id: accountId } = account;
  const begin = useCallback(() => void start(() => client.reloginOAuthAccount(provider, accountId)), [start, client, provider, accountId]);

  useEffect(() => {
    if (!open) return;
    begin();
    return cancel;
  }, [open, begin, cancel]);

  function close() {
    cancel();
    onClose();
  }

  return (
    <Dialog open={open} onClose={close} title={`Sign in again: ${account.label}`}>
      <SignInSteps flow={flow} onRetry={begin} onCancel={close} onDone={close} />
    </Dialog>
  );
}
