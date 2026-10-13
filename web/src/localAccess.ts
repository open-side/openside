import { clearTransport, saveLocalSecret, savePairing, saveToken } from "./auth";

/*
 * Proof that this browser was opened from the computer running the daemon: a per-boot secret
 * the desktop launcher passes in the URL fragment. Only sessionStorage holds it; the daemon
 * invalidates it at each boot. Sign-out, credential rejection and identity changes discard it.
 * It is never put in a QR code or cache, and is sent only on locally authorized API calls.
 */

export interface LocalAccess {
  secret: string;
  /** Admin device token issued for this computer's own browser; absent when only the secret is pasted. */
  token: string | null;
  device: string | null;
}

/**
 * Reads `local_secret`, `local_token` and `local_device` from a URL fragment, a full launch link,
 * or JSON from `openside remote local-access --json`; a bare string is taken as the secret itself.
 */
export function parseLocalAccess(input: string): LocalAccess | null {
  const text = input.trim();
  if (!text) return null;
  if (text.startsWith("{")) {
    try {
      const doc = JSON.parse(text) as { secret?: unknown; token?: unknown; device_id?: unknown };
      if (typeof doc.secret !== "string" || !doc.secret) return null;
      return {
        secret: doc.secret,
        token: typeof doc.token === "string" && doc.token ? doc.token : null,
        device: typeof doc.device_id === "string" && doc.device_id ? doc.device_id : null,
      };
    } catch {
      return null;
    }
  }
  const hash = text.includes("#") ? text.slice(text.indexOf("#") + 1) : text.includes("local_secret=") ? text : null;
  if (hash === null) return /\s/.test(text) ? null : { secret: text, token: null, device: null };
  const params = new URLSearchParams(hash);
  const secret = params.get("local_secret");
  if (!secret) return null;
  return { secret, token: params.get("local_token") || null, device: params.get("local_device") || null };
}

/** Stores the secret (and the local admin token when present). Returns whether a token was stored. */
export function applyLocalAccess(access: LocalAccess, session: Storage = sessionStorage, persistent: Storage = localStorage): boolean {
  if (!access.token) {
    saveLocalSecret(access.secret, session);
    return false;
  }
  // Token changes clear the prior identity's proof; install the new proof only afterward.
  clearTransport(persistent);
  if (access.device) savePairing(access.token, access.device, ["admin"], persistent, session);
  else saveToken(access.token, persistent, session);
  saveLocalSecret(access.secret, session);
  return true;
}

/** Runs once before the app renders: takes the launcher's fragment and removes it from the address bar at once. */
export function consumeLocalAccessFragment(): void {
  const hash = window.location.hash;
  if (!hash.includes("local_secret=")) return;
  const access = parseLocalAccess(hash);
  window.history.replaceState(window.history.state, "", window.location.pathname + window.location.search);
  if (access) applyLocalAccess(access);
}
