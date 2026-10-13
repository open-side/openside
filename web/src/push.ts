import { useEffect } from "react";
import { ApiError, type Client } from "./api/client";
import { desktopRequested, disableDesktop, enableDesktop, extensionAvailable } from "./desktopNotify";

/** The daemon's id for this browser's push subscription; the keys stay in the browser. */
const SUBSCRIPTION_KEY = "openside.push.subscription";
/** Renewed well inside the daemon's 60 s focus TTL (`PUSH_FOCUS_TTL_SECS`). */
const PRESENCE_INTERVAL_MS = 30_000;

export function pushSupported(): boolean {
  return "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
}

export function storedSubscription(): string | null {
  return localStorage.getItem(SUBSCRIPTION_KEY);
}

/** Drops a stored id the daemon no longer knows (subscription deleted, device re-paired). */
export async function subscriptionActive(client: Client): Promise<boolean> {
  const id = storedSubscription();
  if (!id) return false;
  const { subscriptions } = await client.listPushSubscriptions();
  if (subscriptions.some((subscription) => subscription.id === id)) return true;
  localStorage.removeItem(SUBSCRIPTION_KEY);
  return false;
}

/** `applicationServerKey` wants raw bytes; the daemon sends base64url without padding. */
function base64UrlBytes(text: string): Uint8Array<ArrayBuffer> {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  const binary = atob(base64);
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

async function registration(): Promise<ServiceWorkerRegistration> {
  // Production registers sw.js on load; registering again is a no-op and covers dev builds.
  if (!(await navigator.serviceWorker.getRegistration("/"))) await navigator.serviceWorker.register("/sw.js");
  return navigator.serviceWorker.ready;
}

export const PERMISSION_BLOCKED =
  "Notifications are blocked for this site. Click the lock icon in the address bar, set Notifications to Allow, then try again.";
const PUSH_SERVICE_UNAVAILABLE =
  "This browser's push service is not available, and the openside extension is not installed. Install the openside extension to get desktop notifications, or use a browser with Web Push.";

/** Web Push cannot work in this browser; the desktop path may still. Carries the text for when it cannot. */
class PushUnavailable extends Error {}

/** DOMException has a numeric `code`, so the generic ApiError check must not look at it. */
export function describeFailure(cause: unknown): string {
  // Duck-typed: a DOMException from another realm is not `instanceof Error`.
  if (typeof cause === "object" && cause !== null && "message" in cause && typeof cause.message === "string" && cause.message) return cause.message;
  return "Notifications could not be changed. Reload openside and try again.";
}

async function subscribePush(client: Client): Promise<string> {
  if ((await Notification.requestPermission()) !== "granted") throw new PushUnavailable(PERMISSION_BLOCKED);
  const worker = await registration();
  let publicKey: string;
  try {
    publicKey = (await client.vapidKey()).public_key;
  } catch (cause) {
    throw cause instanceof ApiError ? new Error(`The daemon could not provide its push key: ${cause.message}`) : cause;
  }
  let subscription = await worker.pushManager.getSubscription();
  if (!subscription) {
    try {
      subscription = await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: base64UrlBytes(publicKey) });
    } catch (cause) {
      // The patched openside browser has no push-service key, so subscribe() rejects with AbortError.
      if (cause instanceof DOMException && cause.name === "NotAllowedError") throw new PushUnavailable(PERMISSION_BLOCKED);
      if (cause instanceof DOMException && ["AbortError", "NotSupportedError", "InvalidStateError"].includes(cause.name)) throw new PushUnavailable(PUSH_SERVICE_UNAVAILABLE);
      throw cause;
    }
  }
  const { endpoint, keys } = subscription.toJSON();
  if (!endpoint || !keys?.p256dh || !keys.auth) throw new Error("The browser returned an incomplete push subscription.");
  const stored = await client.createPushSubscription({ endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } });
  localStorage.setItem(SUBSCRIPTION_KEY, stored.id);
  // The shell's presence loop only reports every 30 s; without this a push for the screen the
  // person is looking at can arrive right after enabling.
  reportPresence(client);
  return stored.id;
}

export type NotificationMode = "push" | "desktop";

/** Real Web Push where the browser has it; otherwise (or when its push service is missing) the extension's desktop notifications. */
export async function enableNotifications(client: Client): Promise<NotificationMode> {
  if (pushSupported()) {
    try {
      await subscribePush(client);
      return "push";
    } catch (cause) {
      if (!(cause instanceof PushUnavailable)) throw cause;
      if (!(await extensionAvailable())) throw cause;
    }
  }
  await enableDesktop(client);
  return "desktop";
}

export async function disableNotifications(client: Client): Promise<void> {
  if (desktopRequested()) await disableDesktop(client);
  const id = storedSubscription();
  if (id) {
    try {
      await client.deletePushSubscription(id);
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.status === 404)) throw cause;
    }
  }
  localStorage.removeItem(SUBSCRIPTION_KEY);
  const worker = await navigator.serviceWorker?.getRegistration("/");
  await (await worker?.pushManager.getSubscription())?.unsubscribe();
}

function reportPresence(client: Client): void {
  const id = storedSubscription();
  if (!id) return;
  const focused = document.visibilityState === "visible" && document.hasFocus();
  client.pushPresence(id, focused).catch((cause: unknown) => {
    if (cause instanceof ApiError && cause.status === 404) localStorage.removeItem(SUBSCRIPTION_KEY);
  });
}

/**
 * Tells the daemon whether this app is in the foreground, so it does not push what the
 * screen already shows. Runs for the whole signed-in shell.
 */
export function usePushPresence(client: Client): void {
  useEffect(() => {
    const report = () => reportPresence(client);
    report();
    const timer = setInterval(() => {
      if (document.visibilityState === "visible" && document.hasFocus()) report();
    }, PRESENCE_INTERVAL_MS);
    document.addEventListener("visibilitychange", report);
    window.addEventListener("focus", report);
    window.addEventListener("blur", report);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", report);
      window.removeEventListener("focus", report);
      window.removeEventListener("blur", report);
    };
  }, [client]);
}
