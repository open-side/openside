import type { DesktopFailure, DesktopReply, DesktopRequest } from "../../web/src/desktopNotifyProtocol";
import { client, exchangeImportPairing, type Session } from "./api";
import { announceCodes } from "./codes";
import { IMPORT_PAIRING, importCredential, importSecretOf, pairing, retainedImportPairing, type Pairing } from "./storage";

export const SETTLED_NOTIFICATION = "settled:";
// Not a secret, so it lives in local storage and survives a browser restart; the pairing it needs
// does not (session storage), and the web UI re-pairs when it finds the extension unpaired.
const ENABLED = "desktopNotifications";
const STATUSES = "sessionStatuses";
/** The daemon UI is only trusted on this computer; the manifest's externally_connectable matches the same hosts. */
const LOOPBACK_HOSTS = ["127.0.0.1", "localhost"];

export function loopbackOrigin(value: string | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "http:" && LOOPBACK_HOSTS.includes(url.hostname) ? url.origin : null;
  } catch {
    return null;
  }
}

function parseRequest(raw: unknown): DesktopRequest | null {
  if (typeof raw !== "object" || raw === null) return null;
  const message = raw as Record<string, unknown>;
  if (message.type === "openside.desktop.status" || message.type === "openside.desktop.disable") return { type: message.type };
  if (message.type !== "openside.desktop.enable" && message.type !== "openside.desktop.pair") return null;
  if (message.code === undefined) return { type: message.type };
  return typeof message.code === "string" && message.code.length > 0 && message.code.length <= 256 ? { type: message.type, code: message.code } : null;
}

export async function desktopEnabled(): Promise<boolean> {
  return (await chrome.storage.local.get(ENABLED))[ENABLED] === true;
}

function failure(reason: DesktopFailure, message: string): DesktopReply {
  return { ok: false, reason, message };
}

/** Whether the extension may reach this origin; asks only when the request carries the page's user gesture. */
async function hostAccess(origin: string): Promise<boolean> {
  const url = new URL(origin);
  const origins = [`${url.protocol}//${url.hostname}/*`];
  if (await chrome.permissions.contains({ origins })) return true;
  try { return await chrome.permissions.request({ origins }); } catch { return false; }
}

async function importConsent(origin: string, changedApp = false): Promise<boolean> {
  const key = `importConnect:${crypto.randomUUID()}`;
  const { promise, resolve } = Promise.withResolvers<boolean>();
  const changed = (changes: Record<string, chrome.storage.StorageChange>, area: string) => {
    const value: unknown = changes[key]?.newValue;
    if (area === "session" && value && typeof value === "object" && "origin" in value && value.origin === origin && "allowed" in value && typeof value.allowed === "boolean") resolve(value.allowed);
  };
  chrome.storage.onChanged.addListener(changed);
  const timer = setTimeout(() => resolve(false), 120_000);
  try {
    await chrome.storage.session.set({ [key]: { origin, allowed: null, changedApp } });
    await chrome.tabs.create({ url: chrome.runtime.getURL(`import-connect.html#${key.slice("importConnect:".length)}`) });
    return await promise;
  } finally {
    clearTimeout(timer);
    chrome.storage.onChanged.removeListener(changed);
    await chrome.storage.session.remove(key);
  }
}

type Paired = Extract<DesktopReply, { ok: true }>;

export async function handleDesktopRequest(raw: unknown, senderOrigin: string | undefined): Promise<DesktopReply> {
  const origin = loopbackOrigin(senderOrigin);
  const request = parseRequest(raw);
  if (!origin || !request) return failure("refused", "The openside extension only answers the openside web app on this computer.");
  const stored = (await chrome.storage.session.get("pairing")).pairing as Pairing | undefined;
  const here = stored?.url === origin ? stored : undefined;
  const importing = here !== undefined && await importCredential() !== null;
  const state = async (): Promise<Paired> => ({ ok: true, paired: here !== undefined, enabled: here !== undefined && await desktopEnabled(), device: here?.device ?? null, importing });
  if (request.type === "openside.desktop.status") return state();
  if (request.type === "openside.desktop.disable") {
    const reply = await state();
    await chrome.storage.local.set({ [ENABLED]: false });
    await chrome.storage.session.remove(STATUSES);
    // A token the extension minted for this feature goes away with it; one pasted in the options stays,
    // and so does one the import pairing still needs (the page must not revoke it either).
    if (here?.device && !importing) await chrome.storage.session.remove("pairing");
    return { ...reply, enabled: false, device: importing ? null : reply.device };
  }
  const pairForImport = request.type === "openside.desktop.pair";
  if (stored && !here && !pairForImport) return failure("pairing_conflict", `The openside extension is paired with ${stored.url}. Forget that pairing in the extension options, then try again.`);
  let device = here?.device ?? null;
  let replaced: string | null = null;
  let imported = importing;
  if (pairForImport ? !importing : !here) {
    if (!request.code) return failure("needs_pairing", "The openside extension is not paired yet.");
    const previous = pairForImport ? await retainedImportPairing(origin) : null;
    if (pairForImport && !previous && !await importConsent(origin)) return failure("refused", "Not connected. Your import is saved; choose Connect whenever you are ready.");
    if (!await hostAccess(origin)) return failure("permission", "Allow the openside extension to reach this daemon, then try again.");
    try {
      const exchange = pairForImport
        ? await exchangeImportPairing(origin, request.code, previous)
        : { paired: await client({ url: origin, token: "" }).exchangePairing(request.code), continuous: false };
      const paired = exchange.paired;
      try {
        const secret = importSecretOf(paired);
        if (pairForImport && !secret) return failure("exchange_failed", "The daemon did not issue an import secret. Update openside and try again.");
        if (pairForImport && previous && !exchange.continuous && !await importConsent(origin, true)) return failure("refused", "Not connected. Your previous pairing is unchanged.");
        replaced = here?.device ?? null;
        device = paired.device_id;
        await chrome.storage.session.set({ pairing: { url: origin, token: paired.token, device } satisfies Pairing });
        if (pairForImport && secret) await chrome.storage.local.set({ [IMPORT_PAIRING]: { device, secret, origin } });
        else await chrome.storage.local.remove(IMPORT_PAIRING);
        imported = pairForImport;
      } finally {
        paired.token = "";
        paired.import_secret = undefined;
      }
    } catch {
      return failure("exchange_failed", "The daemon did not accept the pairing code. Try again.");
    }
  }
  if (pairForImport) return { ok: true, paired: true, enabled: await desktopEnabled(), device, importing: true, replaced };
  await chrome.storage.local.set({ [ENABLED]: true });
  await chrome.storage.session.remove(STATUSES);
  return { ok: true, paired: true, enabled: true, device, importing: imported };
}

/** The conversation open in the focused window's active tab, if that tab is the daemon UI. */
async function focusedSession(origin: string): Promise<string | null> {
  const window = await chrome.windows.getLastFocused({ populate: true });
  if (!window.focused) return null;
  const url = window.tabs?.find(tab => tab.active)?.url;
  if (!url?.startsWith(`${origin}/sessions/`)) return null;
  return url.slice(origin.length + "/sessions/".length).split(/[/?#]/)[0] || null;
}

/** A run that just ended gets one notification, like the daemon's own push; the first look only records statuses. */
async function announceSettled(sessions: Session[], focused: string | null): Promise<void> {
  const previous = (await chrome.storage.session.get(STATUSES))[STATUSES] as Record<string, string> | undefined;
  await chrome.storage.session.set({ [STATUSES]: Object.fromEntries(sessions.map(session => [session.id, session.status])) });
  if (!previous) return;
  for (const session of sessions) {
    const before = previous[session.id];
    if (session.status !== "idle" || (before !== "running" && before !== "waiting_human") || session.id === focused) continue;
    chrome.notifications.create(`${SETTLED_NOTIFICATION}${session.id}`, {
      type: "basic", iconUrl: "icon.png", title: "Task finished", message: session.title || "Conversation",
    });
  }
}

/** One look at the daemon; resolves to whether a task is running, so the caller can look again soon. */
export async function checkNotifications(): Promise<boolean> {
  const pair = await pairing();
  const api = client(pair);
  const enabled = await desktopEnabled();
  const tickets = (await api.listTickets("pending")).tickets;
  if (!enabled) {
    await announceCodes(tickets);
    return false;
  }
  const focused = await focusedSession(pair.url).catch(() => null);
  await announceCodes(tickets, { all: true, focused: id => id === focused });
  const sessions = (await api.listSessions()).sessions;
  await announceSettled(sessions, focused);
  return sessions.some(session => session.status === "running");
}

export async function openConversation(sessionId: string): Promise<void> {
  const url = `${(await pairing()).url}/sessions/${encodeURIComponent(sessionId)}`;
  const [existing] = await chrome.tabs.query({ url: `${new URL(url).origin}/*` });
  if (existing?.id !== undefined) {
    await chrome.tabs.update(existing.id, { url, active: true });
    await chrome.windows.update(existing.windowId, { focused: true });
  } else {
    await chrome.tabs.create({ url });
  }
}
