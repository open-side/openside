import { useEffect } from "react";
import { ApiError, type Client } from "./api/client";
import { EXTENSION_ID, type DesktopReply, type DesktopRequest } from "./desktopNotifyProtocol";

/*
 * Desktop notifications through the openside MV3 extension, for browsers where Web Push cannot
 * work (the patched openside browser ships no push-service key). The extension's background
 * worker polls the daemon, so notifications arrive with every tab closed; this module only
 * switches it on and off. The extension is addressed by its pinned id over `externally_connectable`.
 */

/** Set while the person wants desktop notifications; holds the device id of the token the extension minted. */
const DESKTOP_KEY = "openside.notify.desktop";
const REPLY_TIMEOUT_MS = 4_000;
/** Redeeming an import pairing waits for the person's click on the extension's consent page (up to 120 s); the code lives 180 s. */
const CONSENT_TIMEOUT_MS = 150_000;

interface ChromeRuntime {
  lastError?: unknown;
  sendMessage(id: string, message: unknown, callback: (reply: unknown) => void): void;
}

/** `chrome.runtime` exists on a page only when an installed extension lists the page in `externally_connectable`. */
function runtime(): ChromeRuntime | undefined {
  const chromeApi: { runtime?: ChromeRuntime } | undefined = Reflect.get(globalThis, "chrome");
  return chromeApi?.runtime;
}

function isReply(value: unknown): value is DesktopReply {
  return typeof value === "object" && value !== null && "ok" in value && typeof value.ok === "boolean";
}

/** `null`: the extension is not installed (or not allowed on this page); nothing answered. */
export function askExtension(request: DesktopRequest): Promise<DesktopReply | null> {
  const bridge = runtime();
  if (!bridge?.sendMessage) return Promise.resolve(null);
  const { promise, resolve } = Promise.withResolvers<DesktopReply | null>();
  const timer = setTimeout(() => resolve(null), request.type === "openside.desktop.pair" && request.code ? CONSENT_TIMEOUT_MS : REPLY_TIMEOUT_MS);
  try {
    bridge.sendMessage(EXTENSION_ID, request, (reply) => {
      clearTimeout(timer);
      // Reading lastError marks it handled; without a listener Chromium logs it as unchecked.
      resolve(bridge.lastError === undefined && isReply(reply) ? reply : null);
    });
  } catch {
    clearTimeout(timer);
    resolve(null);
  }
  return promise;
}

export function desktopRequested(): boolean {
  return localStorage.getItem(DESKTOP_KEY) !== null;
}

/** Whether the extension is installed and answering, without changing anything. */
export async function extensionAvailable(): Promise<boolean> {
  return (await askExtension({ type: "openside.desktop.status" })) !== null;
}

export async function desktopActive(): Promise<boolean> {
  const reply = await askExtension({ type: "openside.desktop.status" });
  return reply?.ok === true && reply.enabled;
}

/**
 * A one-time code for the extension, narrower than this browser's own admin token. Only a code
 * minted for imports asks the daemon to issue an import secret on redemption.
 */
async function mintPairingCode(client: Client, importDelivery: boolean): Promise<string> {
  // A named object, not a literal: `import_delivery` is optional on the request and absent from older generated types.
  const request = { name: "openside extension", scopes: ["read", "steer", "approve", "human_step"] as const, kind: "person_device" as const, via: null, ...(importDelivery ? { import_delivery: true } : {}) };
  try {
    return (await client.createPairing({ ...request, scopes: [...request.scopes] })).code;
  } catch (cause) {
    if (cause instanceof ApiError) throw new Error(`Could not pair the openside extension: ${cause.message}`);
    throw cause;
  }
}

function readDevice(): string | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(DESKTOP_KEY) ?? "null");
    return typeof value === "object" && value !== null && "device" in value && typeof value.device === "string" ? value.device : null;
  } catch {
    return null;
  }
}

/** Turns desktop notifications on, pairing the extension with a fresh one-time code only when it has none. */
export async function enableDesktop(client: Client): Promise<void> {
  let reply = await askExtension({ type: "openside.desktop.enable" });
  if (reply === null) throw new Error("The openside extension is not installed in this browser. Install it to get desktop notifications, or use a browser with Web Push.");
  if (!reply.ok && reply.reason === "needs_pairing") {
    // A restarted browser left the old token behind on the daemon; it is of no use to anyone now.
    const stale = readDevice();
    if (stale) await client.revokeDevice(stale).catch(() => undefined);
    reply = await askExtension({ type: "openside.desktop.enable", code: await mintPairingCode(client, false) });
  }
  if (reply === null) throw new Error("The openside extension did not answer. Reload the extension and try again.");
  if (!reply.ok) throw new Error(reply.message);
  localStorage.setItem(DESKTOP_KEY, JSON.stringify({ device: reply.device }));
}

/**
 * Pairs the extension without touching the notification setting, for features that need the
 * extension to reach the daemon on its own (imports). `null`: the extension is not installed.
 * A failure reply carries its message for the caller to show; a rejection means the daemon
 * would not mint a pairing code.
 */
export async function pairExtension(client: Client): Promise<DesktopReply | null> {
  const status = await askExtension({ type: "openside.desktop.pair" });
  if (status === null || status.ok || status.reason !== "needs_pairing") return status;
  const reply = await askExtension({ type: "openside.desktop.pair", code: await mintPairingCode(client, true) });
  if (reply?.ok) {
    // The token it replaced belonged to this page's own notification pairing, if any.
    if (reply.replaced) await client.revokeDevice(reply.replaced).catch(() => undefined);
    if (desktopRequested()) localStorage.setItem(DESKTOP_KEY, JSON.stringify({ device: reply.device }));
  }
  return reply;
}

export async function disableDesktop(client: Client): Promise<void> {
  const reply = await askExtension({ type: "openside.desktop.disable" });
  // `device: null` from a live extension means it keeps that token for imports; only an extension that did not answer falls back to what this page remembers.
  const device = reply?.ok ? reply.device : readDevice();
  localStorage.removeItem(DESKTOP_KEY);
  if (device) await client.revokeDevice(device).catch(() => undefined);
}

/**
 * The extension keeps its token in session storage, so a browser restart unpairs it while the
 * person's choice survives here. Re-pair quietly when this page finds that state.
 */
export function useDesktopResync(client: Client): void {
  useEffect(() => {
    if (!desktopRequested()) return;
    let cancelled = false;
    void askExtension({ type: "openside.desktop.status" }).then(async (status) => {
      if (cancelled || !status?.ok || (status.paired && status.enabled)) return;
      await enableDesktop(client).catch(() => undefined);
    });
    return () => {
      cancelled = true;
    };
  }, [client]);
}
