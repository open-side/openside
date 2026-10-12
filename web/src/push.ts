import { useEffect } from "react";
import { ApiError, type Client } from "./api/client";

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

export async function enableNotifications(client: Client): Promise<string> {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error("Notifications are blocked for this site. Allow them in the browser's site settings.");
  }
  const worker = await registration();
  const { public_key } = await client.vapidKey();
  const subscription =
    (await worker.pushManager.getSubscription()) ??
    (await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: base64UrlBytes(public_key) }));
  const { endpoint, keys } = subscription.toJSON();
  if (!endpoint || !keys?.p256dh || !keys.auth) throw new Error("The browser returned an incomplete push subscription.");
  const stored = await client.createPushSubscription({ endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } });
  localStorage.setItem(SUBSCRIPTION_KEY, stored.id);
  return stored.id;
}

export async function disableNotifications(client: Client): Promise<void> {
  const id = storedSubscription();
  if (id) {
    try {
      await client.deletePushSubscription(id);
    } catch (cause) {
      if (!(cause instanceof ApiError && cause.status === 404)) throw cause;
    }
  }
  localStorage.removeItem(SUBSCRIPTION_KEY);
  const worker = await navigator.serviceWorker.getRegistration("/");
  await (await worker?.pushManager.getSubscription())?.unsubscribe();
}

/**
 * Tells the daemon whether this app is in the foreground, so it does not push what the
 * screen already shows. Runs for the whole signed-in shell.
 */
export function usePushPresence(client: Client): void {
  useEffect(() => {
    const focused = () => document.visibilityState === "visible" && document.hasFocus();
    const report = () => {
      const id = storedSubscription();
      if (!id) return;
      client.pushPresence(id, focused()).catch((cause: unknown) => {
        if (cause instanceof ApiError && cause.status === 404) localStorage.removeItem(SUBSCRIPTION_KEY);
      });
    };
    report();
    const timer = setInterval(() => {
      if (focused()) report();
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
