import { KeyRound } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Link, useLocation } from "react-router";
import type { VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { ErrorNote } from "../pages/blocks";
import { manageLinkState } from "../pages/manageRoutes";
import { Button, ICON_STROKE, Spinner } from "../ui";

const SECRET_INPUT = { autoComplete: "off", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

/**
 * Passwords can only be imported into the vault, so the import asks for it right where it is
 * needed: set it up, or unlock it, without leaving the guide.
 */
/** What the vault does and does not keep from the agent; shown wherever the vault is offered. */
export const VAULT_TRUST_COPY =
  "The vault stores logins and one-time-code keys encrypted on this computer. Login values are left out of the agent’s tool results, but scripts the agent runs on a page can read its filled fields.";

export function VaultGate({ status, onReady }: { status: VaultStatus; onReady: () => void }) {
  const client = useClient();
  const location = useLocation();
  const [passphrase, setPassphrase] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const setup = status.state === "uninitialized";
  const usesPassphrase = status.backend === "passphrase";
  const mismatch = setup && again.length > 0 && again !== passphrase;

  async function submit(event?: FormEvent) {
    event?.preventDefault();
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
      setBusy(false);
    }
  }

  if (status.state === "unavailable" || status.backend === null) {
    return (
      <div className="onb-vault" role="group" aria-label="Vault">
        <p className="onb-vault-title">
          <KeyRound size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> The vault cannot be used on this computer yet
        </p>
        <p className="onb-note">Passwords stay where they are until it can. You can still import everything else.</p>
        <div className="onb-actions">
          <Link className="ui-button ui-button--compact" to="/vault" state={manageLinkState(location)}>
            See why
          </Link>
        </div>
      </div>
    );
  }

  const disabled = busy || (usesPassphrase && (passphrase.length === 0 || (setup && (again !== passphrase || again.length === 0))));
  return (
    <form className="onb-vault" aria-label="Vault" onSubmit={(event) => void submit(event)}>
      <p className="onb-vault-title">
        <KeyRound size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {setup ? "Passwords go into an encrypted vault" : "Unlock the vault to import passwords"}
      </p>
      <p className="onb-note">{VAULT_TRUST_COPY}</p>
      <p className="onb-note">
        {usesPassphrase
          ? setup
            ? "Choose a passphrase for it. Nobody can recover a forgotten passphrase."
            : "Enter its passphrase."
          : setup
            ? "Its key is kept in this computer’s keyring, so there is nothing to remember."
            : "Its key is in this computer’s keyring."}
      </p>
      {usesPassphrase ? (
        <>
          <label className="onb-field">
            <span>{setup ? "Passphrase" : "Vault passphrase"}</span>
            <input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} {...SECRET_INPUT} />
          </label>
          {setup ? (
            <label className="onb-field">
              <span>Repeat passphrase</span>
              <input type="password" value={again} aria-invalid={mismatch || undefined} onChange={(event) => setAgain(event.target.value)} {...SECRET_INPUT} />
            </label>
          ) : null}
          {mismatch ? <p className="onb-note">The passphrases differ.</p> : null}
        </>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <div className="onb-actions">
        <Button type="submit" variant="primary" disabled={disabled}>
          {busy ? (
            <>
              <Spinner size={16} /> Working…
            </>
          ) : setup ? (
            "Set up the vault"
          ) : (
            "Unlock the vault"
          )}
        </Button>
      </div>
    </form>
  );
}
