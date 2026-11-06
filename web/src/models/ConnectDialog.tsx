import { useCallback, useEffect, useId, useRef, useState, type FormEvent } from "react";
import type { Client } from "../api/client";
import type { ModelAccountRef } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { t, type MessageKey } from "../i18n";
import { ErrorNote } from "../pages/blocks";
import { Button, Dialog, DialogActions, Select, useToast } from "../ui";
import { SECRET_INPUT, SignInSteps, useSignIn, Waiting } from "./OAuthDialogs";
import { ApiKeyGlyph, ProviderGlyph } from "./ProviderGlyph";
import { KEY_PROVIDERS, providerLabel } from "./providers";
import "./models.css";

export interface Subscription {
  /** OAuth provider namespace the sign-in creates an account under. */
  provider: string;
  /** Product name; also the label the new account is stored with. */
  name: string;
  detail?: MessageKey;
}

/** The two subscriptions most people have, as radio cards. */
const FEATURED: Subscription[] = [
  { provider: "openai-codex", name: "ChatGPT", detail: "models.connect.chatgpt" },
  { provider: "anthropic", name: "Claude", detail: "models.connect.claude" },
];

/** Behind "Show more options", in this order; a provider the daemon adds later joins the end. */
const MORE: Subscription[] = [
  { provider: "github-copilot", name: "GitHub Copilot" },
  { provider: "google-gemini-cli", name: "Gemini" },
  { provider: "google-antigravity", name: "Antigravity" },
];

/** Providers a subscription row can be signed in to again by starting a new sign-in. */
export const SIGN_IN_PROVIDERS = new Set([...FEATURED, ...MORE].map((choice) => choice.provider));

/**
 * A new account only serves requests once it is in its provider's pool; the daemon does this for
 * OAuth sign-ins (`join_pool`), and keys get the same treatment here so a saved key works at once.
 */
export async function joinPool(client: Client, provider: string, ref: ModelAccountRef) {
  const { pools } = await client.listPools();
  const current = pools.find((pool) => pool.provider === provider)?.accounts ?? [];
  if (current.some((member) => member.source === ref.source && member.account_id === ref.account_id)) return;
  await client.setPool(provider, [...current, ref]);
}

function KeyField({ value, onChange, id }: { value: string; onChange: (value: string) => void; id: string }) {
  return (
    <label className="field" htmlFor={id}>
      <span>{t("models.apiKey.field")}</span>
      <input id={id} type="password" value={value} onChange={(event) => onChange(event.target.value)} maxLength={4096} required {...SECRET_INPUT} data-1p-ignore data-lpignore="true" />
    </label>
  );
}

/**
 * "Use your own API key": a provider and the key. The value lives only in this form's state and is
 * cleared on save and on leaving; the daemon never sends it back.
 */
function ApiKeyForm({ initialProvider, onBack, onSaved }: { initialProvider?: string; onBack: () => void; onSaved: () => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [provider, setProvider] = useState<string>(initialProvider ?? KEY_PROVIDERS[0]);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const key = value.trim();
    if (!key) return;
    setBusy(true);
    setError(null);
    try {
      const created = await client.addApiKey(provider, { label: providerLabel(provider), key });
      setValue("");
      await joinPool(client, provider, { source: "vault", account_id: created.key_id });
      toast.notify(t("models.apiKey.saved", { provider: providerLabel(provider) }));
      onSaved();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={submit} aria-label={t("models.apiKey.title")}>
      <label className="field" htmlFor={`${id}-provider`}>
        <span>{t("models.apiKey.provider")}</span>
        <Select
          id={`${id}-provider`}
          value={provider}
          onChange={setProvider}
          options={KEY_PROVIDERS.map((candidate) => ({ value: candidate, label: providerLabel(candidate), icon: <ProviderGlyph provider={candidate} /> }))}
        />
      </label>
      <KeyField id={`${id}-key`} value={value} onChange={setValue} />
      <p className="field-help">{t("models.apiKey.help")}</p>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button
          onClick={() => {
            setValue("");
            onBack();
          }}
        >
          {t("common.back")}
        </Button>
        <Button type="submit" variant="primary" disabled={busy || !value.trim()}>
          {t("models.apiKey.save")}
        </Button>
      </DialogActions>
    </form>
  );
}

/** Replaces the key behind an API key row; the account keeps its id and its place. */
export function ReplaceKeyDialog({ open, provider, keyId, name, onClose, onSaved }: { open: boolean; provider: string; keyId: string; name: string; onClose: () => void; onSaved: () => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function close() {
    setValue("");
    setError(null);
    onClose();
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const key = value.trim();
    if (!key) return;
    setBusy(true);
    setError(null);
    try {
      await client.replaceApiKey(provider, keyId, { key });
      setValue("");
      toast.notify(t("models.apiKey.replaced", { name }));
      onSaved();
      close();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={close} title={t("models.apiKey.replaceTitle", { name })} description={t("models.apiKey.help")}>
      <form className="dialog-form" onSubmit={submit} aria-label={t("models.apiKey.replaceTitle", { name })}>
        <KeyField id={id} value={value} onChange={setValue} />
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={close}>{t("common.cancel")}</Button>
          <Button type="submit" variant="primary" disabled={busy || !value.trim()}>
            {t("models.apiKey.save")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/** Where the dialog opens: the picker, or straight into one provider's sign-in or key form. */
export type ConnectStart = { kind: "subscription"; provider: string; name: string } | { kind: "api_key"; provider: string };

type View = { step: "pick" } | { step: "signin"; choice: Subscription } | { step: "key"; provider?: string };

/** The sign-ins the daemon offers, split the way the picker shows them: radio cards, then "More options". */
export function useSubscriptionChoices() {
  const client = useClient();
  const providers = useResource(useCallback(() => client.oauthProviders(), [client]));
  const offered = new Set((providers.data?.providers ?? []).map((info) => info.provider));
  const featured = FEATURED.filter((choice) => offered.has(choice.provider));
  const more: Subscription[] = [
    ...MORE.filter((choice) => offered.has(choice.provider)),
    ...(providers.data?.providers ?? []).filter((info) => !SIGN_IN_PROVIDERS.has(info.provider)).map((info) => ({ provider: info.provider, name: info.name })),
  ];
  return { providers, featured, more };
}

/** The featured subscriptions as radio cards; the guide's subscription step shows the same cards. */
export function SubscriptionChoices({ choices, chosen, onPick }: { choices: Subscription[]; chosen: string | undefined; onPick: (provider: string) => void }) {
  return (
    <div className="models-choices" role="radiogroup" aria-label={t("models.connect.title")}>
      {choices.map((choice) => (
        <label key={choice.provider} className="models-choice">
          <span className="models-choice-glyph">
            <ProviderGlyph provider={choice.provider} size={18} />
          </span>
          <span className="models-choice-copy">
            <span className="models-choice-name">{choice.name}</span>
            {choice.detail ? <span className="models-choice-detail">{t(choice.detail)}</span> : null}
          </span>
          <input type="radio" name="models-connect" value={choice.provider} checked={chosen === choice.provider} onChange={() => onPick(choice.provider)} />
        </label>
      ))}
    </div>
  );
}

/** "More options" (Aside `L 04`): the other sign-ins and "Use your own API key". */
export function MoreOptionsDialog({
  open,
  choices,
  onClose,
  onPick,
  onOwnKey,
}: {
  open: boolean;
  choices: Subscription[];
  onClose: () => void;
  onPick: (choice: Subscription) => void;
  onOwnKey: () => void;
}) {
  return (
    <Dialog open={open} onClose={onClose} title={t("models.connect.moreTitle")} description={t("models.connect.moreDescription")} className="models-connect-more">
      <ul className="models-more-list" aria-label={t("models.connect.moreTitle")}>
        {choices.map((choice) => (
          <li key={choice.provider}>
            <button type="button" className="models-more-row" onClick={() => onPick(choice)}>
              <ProviderGlyph provider={choice.provider} />
              <span>{choice.name}</span>
            </button>
          </li>
        ))}
        <li>
          <button type="button" className="models-more-row" onClick={onOwnKey}>
            <ApiKeyGlyph />
            <span>{t("models.connect.ownKey")}</span>
          </button>
        </li>
      </ul>
      <DialogActions>
        <Button onClick={onClose}>{t("common.cancel")}</Button>
      </DialogActions>
    </Dialog>
  );
}

/**
 * Connect a model provider (Aside's subscription picker, `L 03`/`L 04`): ChatGPT and Claude as radio
 * cards, the rest and "Use your own API key" behind "Show more options". Connect runs the
 * provider's sign-in right here; the new account works as soon as the sign-in is ready.
 */
export function ConnectDialog({ open, start, onClose, onConnected }: { open: boolean; start?: ConnectStart; onClose: () => void; onConnected: () => void }) {
  const client = useClient();
  const { providers, featured, more } = useSubscriptionChoices();
  const [view, setView] = useState<View>({ step: "pick" });
  const [picked, setPicked] = useState<string | null>(null);
  const [showMore, setShowMore] = useState(false);
  const flow = useSignIn(onConnected);
  const { start: startFlow, cancel } = flow;
  const chosen = featured.find((choice) => choice.provider === picked) ?? featured[0];

  const signIn = useCallback(
    (choice: Subscription) => {
      setShowMore(false);
      setView({ step: "signin", choice });
      void startFlow(() => client.createOAuthAccount(choice.provider, { label: choice.name, join_pool: true }));
    },
    [client, startFlow],
  );

  // Reconnect opens the dialog on one provider; that runs once per opening.
  const opened = useRef(false);
  useEffect(() => {
    if (!open) {
      opened.current = false;
      return;
    }
    if (opened.current || !start) return;
    opened.current = true;
    if (start.kind === "subscription") signIn({ provider: start.provider, name: start.name });
    else setView({ step: "key", provider: start.provider });
  }, [open, start, signIn]);

  function close() {
    cancel();
    setView({ step: "pick" });
    setPicked(null);
    setShowMore(false);
    onClose();
  }

  const title =
    view.step === "signin" ? t("models.oauth.connectTitle", { name: view.choice.name }) : view.step === "key" ? t("models.apiKey.title") : t("models.connect.title");

  return (
    <Dialog open={open} onClose={close} title={title} description={view.step === "pick" ? t("models.connect.description") : undefined} className="models-connect">
      {view.step === "signin" ? (
        <SignInSteps flow={flow} onRetry={() => signIn(view.choice)} onCancel={close} onDone={close} />
      ) : view.step === "key" ? (
        <ApiKeyForm
          initialProvider={view.provider}
          onBack={() => (start ? close() : setView({ step: "pick" }))}
          onSaved={() => {
            onConnected();
            close();
          }}
        />
      ) : (
        <form
          className="dialog-form"
          aria-label={t("models.connect.title")}
          onSubmit={(event) => {
            event.preventDefault();
            if (chosen) signIn(chosen);
          }}
        >
          {providers.error ? <ErrorNote>{providers.error}</ErrorNote> : null}
          {providers.data ? (
            <SubscriptionChoices choices={featured} chosen={chosen?.provider} onPick={setPicked} />
          ) : providers.error ? null : (
            <Waiting text={t("models.connect.loading")} />
          )}
          <button type="button" className="models-more-link" onClick={() => setShowMore(true)}>
            {t("models.connect.more")}
          </button>
          <DialogActions>
            <Button onClick={close}>{t("common.cancel")}</Button>
            <Button type="submit" variant="primary" disabled={!chosen}>
              {t("models.connect.action")}
            </Button>
          </DialogActions>
        </form>
      )}
      <MoreOptionsDialog
        open={showMore}
        choices={more}
        onClose={() => setShowMore(false)}
        onPick={signIn}
        onOwnKey={() => {
          setShowMore(false);
          setView({ step: "key" });
        }}
      />
    </Dialog>
  );
}
