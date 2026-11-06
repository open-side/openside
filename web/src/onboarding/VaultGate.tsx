import { KeyRound } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link } from "react-router";
import type { VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { t } from "../i18n";
import { ErrorNote } from "../pages/blocks";
import { Button, ICON_STROKE, Spinner } from "../ui";

const SECRET_INPUT = { autoComplete: "off", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

/**
 * Passwords can only be imported into the vault, so the import asks for it right where it is
 * needed: set it up, or unlock it, without leaving the guide. `bare` drops the gate's own title
 * and box for hosts that already head it (the passwords sheet titles the view with the same words).
 * `onBusy` reports a set-up or unlock request in flight, so a host can hold itself open until it settles.
 */
export function VaultGate({ status, onReady, onBusy, bare = false }: { status: VaultStatus; onReady: () => void; onBusy?: (busy: boolean) => void; bare?: boolean }) {
  const client = useClient();
  const [passphrase, setPassphrase] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // `busy` only disables the button after a re-render; a second Enter before it must not send a second request.
  const sending = useRef(false);

  // The cleanup also reports idle when the gate unmounts mid-request (onReady swaps it out before `finally` runs).
  useEffect(() => {
    onBusy?.(busy);
    return () => onBusy?.(false);
  }, [busy, onBusy]);

  const setup = status.state === "uninitialized";
  const usesPassphrase = status.backend === "passphrase";
  const mismatch = setup && again.length > 0 && again !== passphrase;

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    if (sending.current) return;
    sending.current = true;
    setBusy(true);
    setError(null);
    try {
      const secret = usesPassphrase ? passphrase : undefined;
      await (setup ? client.setupVault(secret) : client.unlockVault(secret));
      setPassphrase("");
      setAgain("");
      onReady();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      sending.current = false;
      setBusy(false);
    }
  }

  if (status.state === "unavailable" || status.backend === null) {
    return (
      <div className="onb-vault" data-bare={bare || undefined} role="group" aria-label={t("onboarding.vault.label")}>
        {bare ? null : (
          <p className="onb-vault-title">
            <KeyRound size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> {t("onboarding.vault.unavailableTitle")}
          </p>
        )}
        <p className="onb-note">{t("onboarding.vault.unavailableNote")}</p>
        <div className="onb-actions">
          <Link className="ui-button" to="/settings/passwords">
            {t("onboarding.vault.seeWhy")}
          </Link>
        </div>
      </div>
    );
  }

  const disabled = busy || (usesPassphrase && (passphrase.length === 0 || (setup && (again !== passphrase || again.length === 0))));
  return (
    <form className="onb-vault" data-bare={bare || undefined} aria-label={t("onboarding.vault.label")} onSubmit={(event) => void submit(event)}>
      {bare ? null : (
        <p className="onb-vault-title">
          <KeyRound size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {setup ? t("onboarding.vault.setupTitle") : t("onboarding.vault.unlockTitle")}
        </p>
      )}
      <p className="onb-note">{t("onboarding.vault.trust")}</p>
      <p className="onb-note">
        {usesPassphrase
          ? setup
            ? t("onboarding.vault.choosePassphrase")
            : t("onboarding.vault.enterPassphrase")
          : setup
            ? t("onboarding.vault.keyringSetup")
            : t("onboarding.vault.keyring")}
      </p>
      {usesPassphrase ? (
        <>
          <label className="onb-field">
            <span>{setup ? t("onboarding.vault.passphrase") : t("onboarding.vault.vaultPassphrase")}</span>
            <input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} {...SECRET_INPUT} />
          </label>
          {setup ? (
            <label className="onb-field">
              <span>{t("onboarding.vault.repeatPassphrase")}</span>
              <input type="password" value={again} aria-invalid={mismatch || undefined} onChange={(event) => setAgain(event.target.value)} {...SECRET_INPUT} />
            </label>
          ) : null}
          {mismatch ? <p className="onb-note">{t("onboarding.vault.mismatch")}</p> : null}
        </>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <div className="onb-actions">
        <Button type="submit" variant="primary" disabled={disabled}>
          {busy ? (
            <>
              <Spinner size={16} /> {t("onboarding.vault.working")}
            </>
          ) : setup ? (
            t("onboarding.vault.setUp")
          ) : (
            t("onboarding.vault.unlock")
          )}
        </Button>
      </div>
    </form>
  );
}
