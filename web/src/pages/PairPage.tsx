import { useEffect, useState } from "react";
import { createClient } from "../api/client";
import { relayTransport, type RelayTarget } from "../api/relay/transport";
import { decodeKey } from "../api/relay/noise";
import type { PairDeviceResponse } from "../api/types";
import { clearTransport, savePairing, saveTransport } from "../auth";
import { errorMessage } from "../hooks/useResource";
import { BrandMark, Spinner } from "../ui";
import "./pages.css";

/**
 * One exchange per code: the code is single-use, and React may run the effect twice
 * (StrictMode, remounts), so a second request would fail with `pairing_not_found`.
 */
const exchanges = new Map<string, Promise<PairDeviceResponse>>();

interface PairingLink {
  code: string;
  /** Set for relay links: the daemon is reached through the relay that served this page. */
  relay: RelayTarget | null;
  error?: string;
}

/**
 * `#<code>` for a daemon reached directly, or `#c=<code>&t=relay&d=<daemon id>&k=<daemon key>`
 * for a daemon behind a relay (the page itself is served by the relay).
 */
function parseFragment(fragment: string): PairingLink {
  try {
    if (!fragment.includes("=")) {
      if (document.querySelector('meta[name="openside-transport"]')?.getAttribute("content") === "relay") {
        return { code: "", relay: null, error: "This relay needs the full pairing link, including the computer's encryption key. Run openside pair --via relay again." };
      }
      return { code: decodeURIComponent(fragment).trim(), relay: null };
    }
    const fields = new URLSearchParams(fragment);
    const code = fields.get("c")?.trim() ?? "";
    const daemonId = fields.get("d");
    const daemonKey = fields.get("k");
    if (fields.get("t") !== "relay" || !daemonId || !daemonKey) {
      return { code: "", relay: null, error: "This pairing link is incomplete or uses an unknown transport. Scan a new QR code from openside pair." };
    }
    if (!decodeKey(daemonKey)) {
      return { code: "", relay: null, error: "This pairing link has an invalid encryption key. Scan a new QR code from openside pair." };
    }
    return { code, relay: { kind: "relay", relay: window.location.origin, daemon_id: daemonId, daemon_key: daemonKey } };
  } catch {
    return { code: "", relay: null, error: "This pairing link is malformed. Scan a new QR code from openside pair." };
  }
}

/** `/pair#…` from the `openside pair` QR code: trade the code for this device's token. */
export function PairPage({ onPaired }: { onPaired: (token: string) => void }) {
  const [{ code, relay, error: linkError }] = useState(() => parseFragment(window.location.hash.slice(1)));
  const relayOrigin = document.querySelector('meta[name="openside-transport"]')?.getAttribute("content") === "relay";
  const [error, setError] = useState<string | null>(
    linkError ?? (code ? null : "This pairing link has no code. Run openside pair on the computer and scan the new QR code."),
  );

  useEffect(() => {
    // A malformed link is still a credential-bearing URL; clear it before showing an error.
    window.history.replaceState(null, "", "/pair");
    if (!code) return;
    let exchange = exchanges.get(code);
    if (!exchange) {
      try {
        const transport = relay ? relayTransport(relay) : null;
        exchange = createClient({ token: () => null, fetch: transport?.fetch, relay: transport !== null }).exchangePairing(code);
        // The pairing session is not reused: the app opens its own once the token is saved.
        void exchange.finally(() => transport?.close()).catch(() => undefined);
      } catch (cause) {
        exchange = Promise.reject(cause instanceof Error ? cause : new Error(String(cause)));
      }
      exchanges.set(code, exchange);
    }
    let active = true;
    exchange.then(
      (paired) => {
        savePairing(paired.token, paired.device_id, paired.scopes);
        if (relay) saveTransport(relay);
        else clearTransport();
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
  }, [code, relay, onPaired]);

  return (
    <main className="token-entry pair-page">
      <div className="token-card">
        <div className="token-brand">
          <BrandMark size={24} />
          <span>openside</span>
        </div>
        <h1>{error ? "Pairing didn’t work" : "Pairing this device"}</h1>
        {error ? (
          <>
            <p role="alert" className="error-note">
              {error}
            </p>
            <p className="token-help">
              Run <code>{relayOrigin || relay ? "openside pair --via relay" : "openside pair"}</code> again on the computer for a new code.
              {!relayOrigin && !relay && !linkError ? <> Or <a href="/">enter a device token instead</a>.</> : null}
            </p>
          </>
        ) : (
          <p role="status" className="loading-row">
            <Spinner size={16} />
            <span>Exchanging the one-time code…</span>
          </p>
        )}
      </div>
    </main>
  );
}
