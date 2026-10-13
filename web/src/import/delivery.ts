import { useEffect } from "react";
import { ApiError, type Client } from "../api/client";
import { EXTENSION_ID } from "../desktopNotifyProtocol";
import { IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN, IMPORT_PERMISSION_STATUS, type ImportPermissionReply, type ImportReply, type ImportStatusReply } from "./protocol";

type DeliveryResult = { delivered: number; pending: number; extensionMissing: boolean; skipped?: number };
interface Runtime {
  lastError?: unknown;
  sendMessage(id: string, message: unknown, callback: (reply: unknown) => void): void;
}
const active = new WeakMap<Client, Promise<DeliveryResult>>();

function askImportExtension(message: unknown): Promise<ImportReply | ImportPermissionReply | ImportStatusReply | null> {
  const chromeApi: { runtime?: Runtime } | undefined = Reflect.get(globalThis, "chrome");
  const runtime = chromeApi?.runtime;
  if (!runtime?.sendMessage) return Promise.resolve(null);
  const { promise, resolve } = Promise.withResolvers<ImportReply | ImportPermissionReply | ImportStatusReply | null>();
  const timer = setTimeout(() => resolve(null), 120_000);
  try {
    runtime.sendMessage(EXTENSION_ID, message, reply => {
      clearTimeout(timer);
      if (runtime.lastError !== undefined || !reply || typeof reply !== "object" || !("ok" in reply)) { resolve(null); return; }
      if (reply.ok === false && "reason" in reply) {
        switch (reply.reason) {
          case "needs_pairing": case "refused": case "invalid_batch": case "permission": case "apply_failed":
            resolve({ ok: false, reason: reply.reason }); return;
        }
      }
      if (reply.ok === true && "device" in reply && typeof reply.device === "string") { resolve({ ok: true, device: reply.device }); return; }
      if (reply.ok === true && "id" in reply && typeof reply.id === "string") {
        if ("granted" in reply && typeof reply.granted === "boolean" && "denied" in reply && typeof reply.denied === "boolean") {
          resolve({ ok: true, id: reply.id, granted: reply.granted, denied: reply.denied }); return;
        }
        if ("bookmarks" in reply && typeof reply.bookmarks === "number" && "history" in reply && typeof reply.history === "number" && "cookies" in reply && typeof reply.cookies === "number") {
          resolve({ ok: true, id: reply.id, bookmarks: reply.bookmarks, history: reply.history, cookies: reply.cookies, ...("skipped" in reply && typeof reply.skipped === "number" ? { skipped: reply.skipped } : {}) }); return;
        }
      }
      resolve(null);
    });
  } catch {
    clearTimeout(timer);
    resolve(null);
  }
  return promise;
}

function pairingRequired(): ApiError {
  return new ApiError(0, "import_delivery_pairing_required", "Connect the openside extension to bring your saved data into this browser.");
}

async function device(): Promise<string | null> {
  const reply = await askImportExtension({ type: IMPORT_STATUS });
  if (!reply) return null;
  if (!reply.ok || !("device" in reply)) throw pairingRequired();
  return reply.device;
}

export function deliverImport(client: Client, onProgress?: (done: number, total: number) => void): Promise<DeliveryResult> {
  const running = active.get(client);
  if (running) return running;
  const run = (async (): Promise<DeliveryResult> => {
    if (client.relay) return { delivered: 0, pending: 0, extensionMissing: false };
    const { batches } = await client.listImportDelivery();
    let delivered = 0;
    let skipped = 0;
    onProgress?.(0, batches.length);
    if (!batches.length) return { delivered, pending: 0, extensionMissing: false };
    const pairedDevice = await device();
    if (!pairedDevice) return { delivered, pending: batches.length, extensionMissing: true };
    for (const batch of batches) {
      const { ticket } = await client.mintImportDeliveryTicket(batch.id, pairedDevice);
      const reply = await askImportExtension({ type: IMPORT_APPLY, ticket });
      if (!reply || (reply.ok && (!("id" in reply) || reply.id !== batch.id || !("bookmarks" in reply)))) return { delivered, pending: batches.length - delivered, extensionMissing: true };
      if (!reply.ok) {
        if (reply.reason === "needs_pairing") throw pairingRequired();
        throw new ApiError(0, reply.reason === "permission" ? "import_delivery_permission_required" : "import_delivery_failed", reply.reason === "permission"
          ? "The import is saved. Choose “Sign in to your sites in openside” to allow access only to the imported sites."
          : "The import is saved, but this browser could not apply it. Reload the openside extension and retry.");
      }
      await client.acknowledgeImportDelivery(batch.id);
      delivered++;
      if ("skipped" in reply && typeof reply.skipped === "number") skipped += reply.skipped;
      onProgress?.(delivered, batches.length);
    }
    return { delivered, pending: 0, extensionMissing: false, ...(skipped ? { skipped } : {}) };
  })();
  active.set(client, run);
  void run.finally(() => active.delete(client)).catch(() => undefined);
  return run;
}

export async function requestImportPermissions(client: Client, onProgress?: (done: number, total: number) => void): Promise<DeliveryResult> {
  if (client.relay) return { delivered: 0, pending: 0, extensionMissing: false };
  const { batches } = await client.listImportDelivery();
  const pairedDevice = await device();
  if (!pairedDevice) return { delivered: 0, pending: batches.length, extensionMissing: true };
  for (const batch of batches) {
    if (!batch.cookie_origins.length) continue;
    const { ticket } = await client.mintImportDeliveryTicket(batch.id, pairedDevice);
    let reply = await askImportExtension({ type: IMPORT_PERMISSION_OPEN, ticket });
    const deadline = Date.now() + 5 * 60_000;
    while (reply?.ok && "granted" in reply && !reply.granted && !reply.denied && Date.now() < deadline) {
      const id = reply.id;
      const { promise, resolve } = Promise.withResolvers<void>();
      setTimeout(resolve, 500);
      await promise;
      reply = await askImportExtension({ type: IMPORT_PERMISSION_STATUS, id });
    }
    if (!reply) return { delivered: 0, pending: batches.length, extensionMissing: true };
    if (!reply.ok && reply.reason === "needs_pairing") throw pairingRequired();
    if (!reply.ok || !("granted" in reply) || !reply.granted) {
      throw new ApiError(0, "import_delivery_permission_required", "Site access was not allowed. Your import is saved; you can retry whenever you are ready.");
    }
  }
  return deliverImport(client, onProgress);
}

export function useDeliverPending(client: Client): void {
  useEffect(() => {
    if (client.relay || !["127.0.0.1", "localhost"].includes(window.location.hostname)) return;
    const resume = () => {
      // Launcher authority may be absent or stale; automatic delivery must never block the app shell.
      void deliverImport(client).catch(() => undefined);
    };
    const visible = () => { if (document.visibilityState === "visible") resume(); };
    resume();
    window.addEventListener("online", resume);
    document.addEventListener("visibilitychange", visible);
    return () => {
      window.removeEventListener("online", resume);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [client]);
}
