import { useState, type FormEvent } from "react";
import { ApiError, createClient } from "../api/client";
import { CliHint } from "../pages/CliHint";
import "../pages/pages.css";
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
      setError(cause instanceof ApiError ? cause.message : "Could not verify the token");
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
        <h1>Connect this device</h1>
        <p>{relay
          ? "Pair this device using a QR code or link from the computer running openside. A token alone cannot identify the computer or its encryption key."
          : "Paste a device token paired on the computer running the openside daemon."}</p>
        {relay ? (
          <>
          {error ? <p role="alert" className="error-note">{error}</p> : null}
          <p className="token-help">
            On that computer, open openside and go to Settings → Remote access → Connect a phone, then scan the QR code or open the full pairing link on this device.
          </p>
          <CliHint command="openside pair --via relay" />
          </>
        ) : <form onSubmit={submit}>
          <label className="field">
            <span>Device token</span>
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="Paste your device token"
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
            {busy ? "Connecting…" : "Connect"}
          </Button>
        </form>}
        {!relay ? <>
          <p className="token-help">
            No token yet? On that computer, open openside and go to Settings → Remote access → Connect a phone, then scan the QR code with this device.
          </p>
          <CliHint command="openside pair" />
        </> : null}
      </div>
    </main>
  );
}
