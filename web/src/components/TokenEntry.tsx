import { useState, type FormEvent } from "react";
import { ApiError, createClient } from "../api/client";
import "../pages/pages.css";
import { BrandMark, Button } from "../ui";

/** Accepts a pasted device token after the daemon confirms it with `GET /v1/health`. */
export function TokenEntry({ onToken, reason }: { onToken: (token: string) => void; reason?: string }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(reason ?? null);

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
          <BrandMark size={28} />
          <h1>openside</h1>
        </div>
        <p>Paste a device token paired on the computer running the openside daemon.</p>
        <form onSubmit={submit}>
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
            />
          </label>
          <Button type="submit" variant="primary" disabled={busy || !token.trim()}>
            {busy ? "Connecting…" : "Connect"}
          </Button>
        </form>
        {error ? (
          <p role="alert" className="error-note">
            {error}
          </p>
        ) : null}
        <p>
          No token yet? Run <code>openside pair</code> on that computer and scan the QR code with this device.
        </p>
      </div>
    </main>
  );
}
