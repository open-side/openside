import { useState, type FormEvent } from "react";
import { ApiError, createClient } from "../api/client";
import { t } from "../i18n";
import { CliHint } from "../pages/CliHint";
import "../pages/pages.css";
import "../pages/signin.css";
import { BrandMark, Button } from "../ui";

/** Accepts a pasted device token after the daemon confirms it with `GET /v1/health`. */
export function TokenEntry({ onToken, reason, pairingRequired = false }: { onToken: (token: string) => void; reason?: string; pairingRequired?: boolean }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(reason ?? null);
  const relay = pairingRequired || document.querySelector('meta[name="openside-transport"]')?.getAttribute("content") === "relay";

  async function submit(event: FormEvent) {
    event.preventDefault();
    const candidate = token.trim();
    if (!candidate) return;
    setBusy(true);
    setError(null);
    try {
      await createClient({ token: () => candidate }).health();
      onToken(candidate);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : t("shell.token.verifyFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="token-entry">
      <div className="token-card">
        <div className="token-brand">
          <BrandMark size={24} />
          <span>openside</span>
        </div>
        <h1>{t("shell.token.title")}</h1>
        <p>{relay ? t("shell.token.relayIntro") : t("shell.token.intro")}</p>
        {relay ? (
          <>
          {error ? <p role="alert" className="error-note">{error}</p> : null}
          <p className="token-help">
            {t("shell.token.relayHelp")}
          </p>
          <CliHint command="openside pair --via relay" />
          </>
        ) : <form onSubmit={submit}>
          <label className="field">
            <span>{t("shell.token.label")}</span>
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder={t("shell.token.placeholder")}
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              disabled={busy}
              aria-invalid={error ? true : undefined}
            />
          </label>
          {error ? (
            <p role="alert" className="error-note">
              {error}
            </p>
          ) : null}
          <Button type="submit" variant="primary" disabled={busy || !token.trim()}>
            {busy ? t("shell.token.connecting") : t("shell.token.connect")}
          </Button>
        </form>}
        {!relay ? <>
          <p className="token-help">
            {t("shell.token.help")}
          </p>
          <CliHint command="openside pair" />
        </> : null}
      </div>
    </main>
  );
}
