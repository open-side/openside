import { useEffect, useState } from "react";
import { createClient } from "../api/client";
import type { PairDeviceResponse } from "../api/types";
import { savePairing } from "../auth";
import { errorMessage } from "../hooks/useResource";
import { BrandMark, Spinner } from "../ui";
import "./pages.css";

/**
 * One exchange per code: the code is single-use, and React may run the effect twice
 * (StrictMode, remounts), so a second request would fail with `pairing_not_found`.
 */
const exchanges = new Map<string, Promise<PairDeviceResponse>>();

/** `/pair#<code>` from the `openside pair` QR code: trade the code for this device's token. */
export function PairPage({ onPaired }: { onPaired: (token: string) => void }) {
  const [code] = useState(() => decodeURIComponent(window.location.hash.slice(1)).trim());
  const [error, setError] = useState<string | null>(
    code ? null : "This pairing link has no code. Run openside pair on the computer and scan the new QR code.",
  );

  useEffect(() => {
    if (!code) return;
    // The code is a credential until it is used: keep it out of the address bar and history.
    window.history.replaceState(null, "", "/pair");
    let exchange = exchanges.get(code);
    if (!exchange) {
      exchange = createClient({ token: () => null }).exchangePairing(code);
      exchanges.set(code, exchange);
    }
    let active = true;
    exchange.then(
      (paired) => {
        savePairing(paired.token, paired.device_id, paired.scopes);
        window.history.replaceState(null, "", "/");
        if (active) onPaired(paired.token);
      },
      (cause: unknown) => {
        if (active) setError(errorMessage(cause));
      },
    );
    return () => {
      active = false;
    };
  }, [code, onPaired]);

  return (
    <main className="token-entry pair-page">
      <div className="token-card">
        <div className="token-brand">
          <BrandMark size={28} />
          <h1>openside</h1>
        </div>
        {error ? (
          <>
            <p role="alert" className="error-note">
              {error}
            </p>
            <a href="/">Enter a device token instead</a>
          </>
        ) : (
          <p role="status" className="loading-row">
            <Spinner size={16} />
            <span>Pairing this device…</span>
          </p>
        )}
      </div>
    </main>
  );
}
