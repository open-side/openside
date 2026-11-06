import { client } from "./api";
import { ApiError } from "../../web/src/api/client";
import { validateDaemonUrl, type Pairing } from "./storage";
import { trustedDaemonOrigin } from "./daemonOrigin";

/**
 * tools/openside-browser writes this file next to the unpacked extension's own files on every
 * launch. It is not a web_accessible_resource, so pages cannot read it; the code inside is
 * one-time, lives 180 s, and the daemon accepts it only from this extension's origin.
 */
export const LAUNCH_FILE = "launch-pairing.json";
/** Extension pages ask the worker to pair from the launcher's file before offering manual pairing. */
export const ADOPT_LAUNCHER_PAIRING = "openside.launcher.adopt";
/** Digest of the last launcher code this browser session tried; a code is spent by its first exchange attempt. */
const SPENT = "launcherCodeSpent";

interface LaunchOffer { url: string; code: string }

function parseOffer(value: unknown): LaunchOffer | null {
  if (typeof value !== "object" || value === null) return null;
  const { url, code } = value as Record<string, unknown>;
  if (typeof url !== "string" || typeof code !== "string" || !/^[0-9a-f]{64}$/.test(code)) return null;
  try {
    // Only the daemon the manifest names: its exact origin is the one granted without a prompt.
    const origin = trustedDaemonOrigin(validateDaemonUrl(url));
    return origin ? { url: origin, code } : null;
  } catch {
    return null;
  }
}

async function digest(code: string): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(code)));
  return Array.from(hash, byte => byte.toString(16).padStart(2, "0")).join("");
}

async function stored(): Promise<Pairing | undefined> {
  return (await chrome.storage.session.get("pairing")).pairing as Pairing | undefined;
}

async function adopt(): Promise<boolean> {
  if (await stored()) return true;
  let offer: LaunchOffer | null;
  try {
    const response = await fetch(chrome.runtime.getURL(LAUNCH_FILE), { cache: "no-store" });
    offer = response.ok ? parseOffer(await response.json()) : null;
  } catch {
    return false;
  }
  if (!offer) return false;
  const spent = await digest(offer.code);
  // Retrying a spent code would only count as a failed guess against the daemon's pairing budget.
  if ((await chrome.storage.session.get(SPENT))[SPENT] === spent) return false;
  await chrome.storage.session.set({ [SPENT]: spent });
  let paired;
  try {
    paired = await client({ url: offer.url, token: "" }).exchangePairing(offer.code);
  } catch {
    return false;
  }
  try {
    // A pairing that arrived meanwhile (a pasted token, the web app's code) was the person's choice; keep it.
    if (await stored()) return true;
    await chrome.storage.session.set({ pairing: { url: offer.url, token: paired.token, device: paired.device_id, launcher: true } satisfies Pairing });
    return true;
  } finally {
    paired.token = "";
  }
}

let pending: Promise<boolean> | undefined;

/** Pairs from the launcher's one-time code when nothing is paired yet; resolves to whether a pairing exists now. */
export function adoptLauncherPairing(): Promise<boolean> {
  pending ??= adopt().finally(() => { pending = undefined; });
  return pending;
}

/** A launcher pairing revoked in Devices is dropped, so the next launch can pair again; true when it was. */
export async function forgetRevokedLauncherPairing(error: unknown): Promise<boolean> {
  if (!(error instanceof ApiError) || error.status !== 401) return false;
  const current = await stored();
  if (!current?.launcher) return false;
  await chrome.storage.session.remove("pairing");
  return true;
}
