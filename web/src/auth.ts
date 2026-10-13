import type { RelayTarget } from "./api/relay/transport";

/*
 * Token storage decision: the device token lives in localStorage so an installed
 * PWA survives app restarts without re-pairing (sessionStorage is wiped whenever a
 * phone kills the app). The risk is script access to storage; it is contained by
 * the daemon CSP (no inline or third-party script, connect-src 'self'), no
 * third-party dependencies at runtime beyond React, and "Sign out" removes it.
 * The socket-only launcher puts the token in a transient fragment that is stripped
 * immediately. Tokens never enter logs or the service worker cache, and cookies
 * are not used, so cross-site requests cannot carry them (no cookie CSRF surface).
 */
const STORAGE_KEY = "openside.token";
const LOCAL_SECRET_KEY = "openside.local-secret";

export function loadLocalSecret(storage: Storage = sessionStorage): string | null {
  return storage.getItem(LOCAL_SECRET_KEY);
}

export function saveLocalSecret(secret: string, storage: Storage = sessionStorage): void {
  storage.setItem(LOCAL_SECRET_KEY, secret);
}

export function clearLocalSecret(storage: Storage = sessionStorage): void {
  storage.removeItem(LOCAL_SECRET_KEY);
}

export function loadToken(storage: Storage = localStorage): string | null {
  return storage.getItem(STORAGE_KEY);
}

export function saveToken(token: string, storage: Storage = localStorage, session: Storage = sessionStorage): void {
  if (loadToken(storage) !== token) {
    clearLocalSecret(session);
    storage.removeItem(DEVICE_KEY);
    storage.removeItem(TRANSPORT_KEY);
  }
  storage.setItem(STORAGE_KEY, token);
  // A pasted token's scopes are unknown; never keep the previous pairing's.
  storage.removeItem(SCOPES_KEY);
}

export function clearToken(storage: Storage = localStorage, session: Storage = sessionStorage): void {
  clearLocalSecret(session);
  storage.removeItem(STORAGE_KEY);
  storage.removeItem(DEVICE_KEY);
  storage.removeItem(SCOPES_KEY);
  storage.removeItem(TRANSPORT_KEY);
}

/** Device id from `/pair`, so the Devices screen can mark (and warn before revoking) this device. */
const DEVICE_KEY = "openside.device";
/** Scopes cached from pairing or the daemon's `/v1/devices/me` response. */
const SCOPES_KEY = "openside.scopes";

export function thisDevice(storage: Storage = localStorage): string | null {
  return storage.getItem(DEVICE_KEY);
}

/**
 * Whether this device's token holds `scope` (admin holds every scope), or `undefined` when the
 * token was pasted and its scopes are unknown.
 */
export function mayUse(scope: "approve" | "steer" | "admin", storage: Storage = localStorage): boolean | undefined {
  const stored = storage.getItem(SCOPES_KEY);
  if (stored === null) return undefined;
  try {
    const scopes: unknown = JSON.parse(stored);
    return Array.isArray(scopes) ? scopes.includes(scope) || scopes.includes("admin") : undefined;
  } catch {
    return undefined;
  }
}

/** Scopes the daemon reports for this device after an approved access request. */
export function saveScopes(scopes: readonly string[], storage: Storage = localStorage): void {
  storage.setItem(SCOPES_KEY, JSON.stringify(scopes));
}

export function savePairing(token: string, deviceId: string, scopes: readonly string[], storage: Storage = localStorage, session: Storage = sessionStorage): void {
  saveToken(token, storage, session);
  storage.setItem(DEVICE_KEY, deviceId);
  storage.setItem(SCOPES_KEY, JSON.stringify(scopes));
}

/*
 * How this device reaches the daemon when it is not the page's own origin: set by `/pair`
 * from a relay pairing link and removed with the token. It holds no secret (relay origin,
 * daemon routing id and public key).
 */
const TRANSPORT_KEY = "openside.transport";

export function saveTransport(target: RelayTarget, storage: Storage = localStorage): void {
  storage.setItem(TRANSPORT_KEY, JSON.stringify(target));
}

export function loadTransport(storage: Storage = localStorage): RelayTarget | null {
  const stored = storage.getItem(TRANSPORT_KEY);
  if (!stored) return null;
  let value: Partial<RelayTarget>;
  try {
    value = JSON.parse(stored) as Partial<RelayTarget>;
  } catch {
    throw new Error("Invalid saved relay connection. Pair this device again.");
  }
  if (value?.kind !== "relay" || typeof value.relay !== "string" || typeof value.daemon_id !== "string" || typeof value.daemon_key !== "string") {
    throw new Error("Invalid saved relay connection. Pair this device again.");
  }
  return value as RelayTarget;
}

export function clearTransport(storage: Storage = localStorage): void {
  storage.removeItem(TRANSPORT_KEY);
}
