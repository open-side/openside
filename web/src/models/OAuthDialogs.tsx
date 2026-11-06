import { Copy, ExternalLink } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { ApiError, apiErrorMessage, type Client } from "../api/client";
import type { OAuthTransaction, OAuthTransactionStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { t, type MessageKey } from "../i18n";
import { ErrorNote } from "../pages/blocks";
import { formatRelative } from "../pages/format";
import { Button, copyText, Dialog, DialogActions, IconButton, ICON_STROKE, Spinner, useToast } from "../ui";

/** Keeps password managers and autocorrect away from pasted codes and keys. */
export const SECRET_INPUT = { autoComplete: "new-password", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

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
const REASONS: Record<string, MessageKey> = {
  oauth_callback_busy: "models.oauth.reason.oauth_callback_busy",
  oauth_invalid_grant: "models.oauth.reason.oauth_invalid_grant",
  oauth_failed: "models.oauth.reason.oauth_failed",
  oauth_flow_unsupported: "models.oauth.reason.oauth_flow_unsupported",
  oauth_helper_unavailable: "models.oauth.reason.oauth_helper_unavailable",
  oauth_cancelled: "models.oauth.reason.oauth_cancelled",
  oauth_expired: "models.oauth.reason.oauth_expired",
  oauth_account_removed: "models.oauth.reason.oauth_account_removed",
};

function reasonCopy(tx: OAuthTransaction): string {
  const reason = tx.reason ?? "";
  const key = REASONS[reason];
  return key ? t(key) : (apiErrorMessage(reason) ?? t(tx.status === "expired" ? "models.oauth.reason.oauth_expired" : "models.oauth.reason.oauth_failed"));
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
export interface SignIn {
  tx: OAuthTransaction | null;
  /** A start or paste the daemon refused; transaction failures are in `tx.reason`. */
  error: string | null;
  busy: boolean;
  start: (begin: () => Promise<OAuthTransaction>) => Promise<void>;
  cancel: () => void;
  submit: (code: string) => Promise<boolean>;
}

export function useSignIn(onReady: (tx: OAuthTransaction) => void): SignIn {
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
      onClick={() => void copyText(value).then((copied) => toast.notify(copied ? done : t("models.oauth.copyFailed"), copied ? undefined : "error"))}
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
    <form className="oauth-manual" onSubmit={submit} aria-label={t("models.oauth.manual.form")}>
      <label className="field">
        <span>{t("models.oauth.manual.label")}</span>
        <input value={value} onChange={(event) => setValue(event.target.value)} maxLength={4096} {...SECRET_INPUT} data-1p-ignore data-lpignore="true" aria-describedby="oauth-manual-help" />
        <small id="oauth-manual-help" className="field-help">
          {t("models.oauth.manual.help")}
        </small>
      </label>
      <Button type="submit" disabled={sending || !value.trim()}>
        {sending ? t("models.oauth.manual.sending") : t("common.send")}
      </Button>
    </form>
  );
}

/** Where a running sign-in stands, and what the person does next. */
export function SignInSteps({ flow, onRetry, onCancel, onDone }: { flow: SignIn; onRetry: () => void; onCancel: () => void; onDone: () => void }) {
  const client = useClient();
  const { tx, error, busy } = flow;

  if (!tx) {
    return (
      <div className="dialog-form">
        {error ? <ErrorNote>{error}</ErrorNote> : <Waiting text={t("models.oauth.starting")} />}
        <DialogActions>
          <Button onClick={onCancel}>{error ? t("common.close") : t("common.cancel")}</Button>
          {error ? (
            <Button variant="primary" disabled={busy} onClick={onRetry}>
              {t("common.tryAgain")}
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
          {tx.account?.label ? t("models.oauth.readyNamed", { name: tx.account.label }) : t("models.oauth.ready")}
        </p>
        <DialogActions>
          <Button variant="primary" onClick={onDone}>
            {t("common.done")}
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
          <Button onClick={onDone}>{t("common.close")}</Button>
          <Button variant="primary" disabled={busy} onClick={onRetry}>
            {t("common.tryAgain")}
          </Button>
        </DialogActions>
      </div>
    );
  }

  const authorization = tx.authorization;
  const device = tx.method === "device";
  return (
    <div className="dialog-form">
      {tx.status === "starting" || !authorization ? <Waiting text={t("models.oauth.starting")} /> : null}
      {tx.status === "exchanging" ? <Waiting text={t("models.oauth.finishing")} /> : null}
      {authorization && (tx.status === "awaiting_browser" || tx.status === "awaiting_code") ? (
        <>
          <p className="settings-text">
            {device ? t("models.oauth.deviceSteps") : t("models.oauth.browserSteps")}
          </p>
          {device && authorization.user_code ? (
            <span className="login-command login-command--block oauth-user-code">
              <code aria-label={t("models.oauth.code")}>{authorization.user_code}</code>
              <CopyButton value={authorization.user_code} label={t("models.oauth.copyCode")} done={t("models.oauth.codeCopied")} />
            </span>
          ) : null}
          {!device && authorization.instructions ? <p className="settings-text">{authorization.instructions}</p> : null}
          <span className="oauth-open">
            <a className="ui-button ui-button--primary" href={authorization.url} target="_blank" rel="noopener noreferrer">
              <ExternalLink size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("models.oauth.openPage")}
            </a>
            <CopyButton value={authorization.url} label={t("models.oauth.copyLink")} done={t("models.oauth.linkCopied")} />
          </span>
          {!device && onOtherDevice(client) ? (
            <p className="field-help oauth-note" role="note">
              {t("models.oauth.otherDevice")}
            </p>
          ) : null}
          {tx.manual_input ? <ManualInput onSubmit={flow.submit} /> : null}
          <Waiting text={t("models.oauth.waiting", { when: formatRelative(tx.expires_at_ms) })} />
        </>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>{t("common.cancel")}</Button>
      </DialogActions>
    </div>
  );
}

export function Waiting({ text }: { text: string }) {
  return (
    <p className="settings-text signin-wait oauth-wait" role="status">
      <Spinner size={14} />
      {text}
    </p>
  );
}

/** Signs an existing account in again; the account keeps its id, so nothing else changes. */
export function ReloginDialog({
  open,
  provider,
  accountId,
  name,
  onClose,
  onReady,
}: {
  open: boolean;
  provider: string;
  accountId: string;
  /** The provider row's name, for the title. */
  name: string;
  onClose: () => void;
  onReady: () => void;
}) {
  const client = useClient();
  const flow = useSignIn(onReady);
  const { start, cancel } = flow;
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
    <Dialog open={open} onClose={close} title={t("models.oauth.reloginTitle", { name })}>
      <SignInSteps flow={flow} onRetry={begin} onCancel={close} onDone={close} />
    </Dialog>
  );
}
