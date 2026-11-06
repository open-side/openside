import { useEffect, useState } from "react";
import { createClient } from "../api/client";
import { relayTransport, type RelayTarget } from "../api/relay/transport";
import { decodeKey } from "../api/relay/noise";
import type { PairDeviceResponse } from "../api/types";
import { clearTransport, savePairing, saveTransport } from "../auth";
import { errorMessage } from "../hooks/useResource";
import { t } from "../i18n";
import { tNodes } from "../i18n/react";
import { BrandMark, Spinner } from "../ui";
import { CliHint } from "./CliHint";
import "./pages.css";
import "./signin.css";

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
        return { code: "", relay: null, error: t("settings.pair.error.relayNeedsFullLink") };
      }
      return { code: decodeURIComponent(fragment).trim(), relay: null };
    }
    const fields = new URLSearchParams(fragment);
    const code = fields.get("c")?.trim() ?? "";
    const daemonId = fields.get("d");
    const daemonKey = fields.get("k");
    if (fields.get("t") !== "relay" || !daemonId || !daemonKey) {
      return { code: "", relay: null, error: t("settings.pair.error.incomplete") };
    }
    if (!decodeKey(daemonKey)) {
      return { code: "", relay: null, error: t("settings.pair.error.invalidKey") };
    }
    return { code, relay: { kind: "relay", relay: window.location.origin, daemon_id: daemonId, daemon_key: daemonKey } };
  } catch {
    return { code: "", relay: null, error: t("settings.pair.error.malformed") };
  }
}

/** `/pair#…` from a pairing QR code or link: trade the code for this device's token. */
export function PairPage({ onPaired }: { onPaired: (token: string) => void }) {
  const [{ code, relay, error: linkError }] = useState(() => parseFragment(window.location.hash.slice(1)));
  const relayOrigin = document.querySelector('meta[name="openside-transport"]')?.getAttribute("content") === "relay";
  const [error, setError] = useState<string | null>(linkError ?? (code ? null : t("settings.pair.error.noCode")));

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
        <h1>{error ? t("settings.pair.failedTitle") : t("settings.pair.title")}</h1>
        {error ? (
          <>
            <p role="alert" className="error-note">
              {error}
            </p>
            <p className="token-help">
              {t("settings.pair.help")}
              {!relayOrigin && !relay && !linkError ? <> {tNodes("settings.pair.orToken", { link: <a href="/">{t("settings.pair.tokenLink")}</a> })}</> : null}
            </p>
            <CliHint command={relayOrigin || relay ? "openside pair --via relay" : "openside pair"} />
          </>
        ) : (
          <p role="status" className="loading-row">
            <Spinner size={16} />
            <span>{t("settings.pair.exchanging")}</span>
          </p>
        )}
      </div>
    </main>
  );
}
