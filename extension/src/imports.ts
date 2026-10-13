import { IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN, IMPORT_PERMISSION_STATUS, type ImportDeliveryBatch, type ImportPermissionReply, type ImportReply, type ImportStatusReply } from "../../web/src/import/protocol";
import { loopbackOrigin } from "./desktop";
import { importCredential, type ImportCredential } from "./storage";

const BRANDS: Record<string, string> = { chrome: "Chrome", chromium: "Chromium", brave: "Brave", edge: "Edge", vivaldi: "Vivaldi", opera: "Opera", arc: "Arc", firefox: "Firefox", librewolf: "LibreWolf", zen: "Zen", floorp: "Floorp", safari: "Safari" };
let applying: Promise<unknown> = Promise.resolve();

function webUrl(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try { return ["http:", "https:"].includes(new URL(value).protocol); } catch { return false; }
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function bookmarkUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const url = new URL(value);
    return ["http:", "https:", "file:", "ftp:"].includes(url.protocol) ? url.href : null;
  } catch { return null; }
}

function validBatch(value: unknown): value is ImportDeliveryBatch {
  if (!record(value)) return false;
  return typeof value.id === "string" && !!value.id
    && record(value.source) && [value.source.id, value.source.brand, value.source.name].every(part => typeof part === "string")
    && Array.isArray(value.bookmarks)
    && Array.isArray(value.history) && value.history.every(visit => record(visit) && webUrl(visit.url))
    && Array.isArray(value.cookies) && value.cookies.every(cookie => record(cookie) && webUrl(cookie.url) && typeof cookie.name === "string" && typeof cookie.value === "string" && typeof cookie.path === "string" && typeof cookie.secure === "boolean" && typeof cookie.httpOnly === "boolean"
      && (cookie.domain == null || typeof cookie.domain === "string")
      && (cookie.expirationDate == null || (typeof cookie.expirationDate === "number" && Number.isFinite(cookie.expirationDate)))
      && (cookie.sameSite == null || (typeof cookie.sameSite === "string" && ["unspecified", "no_restriction", "lax", "strict"].includes(cookie.sameSite)))
      && (cookie.partitionKey == null || (record(cookie.partitionKey) && webUrl(cookie.partitionKey.topLevelSite) && (cookie.partitionKey.hasCrossSiteAncestor == null || typeof cookie.partitionKey.hasCrossSiteAncestor === "boolean"))));
}

export function cookieDetails(cookie: ImportDeliveryBatch["cookies"][number]): chrome.cookies.SetDetails {
  const details: chrome.cookies.SetDetails = { url: cookie.url, name: cookie.name, value: cookie.value, path: cookie.path, secure: cookie.secure, httpOnly: cookie.httpOnly };
  if (cookie.domain != null) details.domain = cookie.domain;
  if (cookie.sameSite != null) details.sameSite = cookie.sameSite as chrome.cookies.SameSiteStatus;
  if (cookie.expirationDate != null) details.expirationDate = cookie.expirationDate;
  if (cookie.partitionKey != null) {
    details.partitionKey = { topLevelSite: cookie.partitionKey.topLevelSite };
    if (cookie.partitionKey.hasCrossSiteAncestor != null) {
      const version = Number(/(?:Chrome|Chromium)\/(\d+)/.exec(navigator.userAgent)?.[1]);
      // Dropping this bit would move a partitioned cookie into a different security boundary.
      if (version < 130 || !version) throw new Error("Partitioned cookie needs Chrome 130 or newer.");
      details.partitionKey.hasCrossSiteAncestor = cookie.partitionKey.hasCrossSiteAncestor;
    }
  }
  return details;
}

async function applyBatch(batch: ImportDeliveryBatch): Promise<ImportReply> {
  let bookmarks = 0;
  let history = 0;
  let cookies = 0;
  let skipped = 0;
  const supported: { bookmark: ImportDeliveryBatch["bookmarks"][number]; url: string }[] = [];
  for (const bookmark of batch.bookmarks) {
    if (!record(bookmark) || typeof bookmark.title !== "string" || !Array.isArray(bookmark.folder) || !bookmark.folder.every(part => typeof part === "string")) { skipped++; continue; }
    const url = bookmarkUrl(bookmark.url);
    if (!url) { skipped++; continue; }
    supported.push({ bookmark, url });
  }
  for (const cookie of batch.cookies) {
    const url = new URL(cookie.url);
    if (!await chrome.permissions.contains({ origins: [`${url.protocol}//${url.hostname}/*`] })) return { ok: false, reason: "permission" };
  }
  if (supported.length) {
    const roots = await chrome.bookmarks.getChildren("0");
    const parent = roots.find(node => node.folderType === "other" && !node.unmodifiable)
      ?? roots.find(node => node.id === "2" && !node.unmodifiable)
      ?? roots.find(node => !node.url && !node.unmodifiable);
    if (!parent) return { ok: false, reason: "apply_failed" };
    const children = new Map<string, chrome.bookmarks.BookmarkTreeNode[]>();
    const getChildren = async (id: string) => {
      let list = children.get(id);
      if (!list) { list = await chrome.bookmarks.getChildren(id); children.set(id, list); }
      return list;
    };
    const folder = async (parentId: string, title: string) => {
      const list = await getChildren(parentId);
      const found = list.find(node => !node.url && !node.unmodifiable && node.title === title);
      if (found) return found.id;
      const created = await chrome.bookmarks.create({ parentId, title });
      list.push(created);
      children.set(created.id, []);
      return created.id;
    };
    const root = await folder(parent.id, `Imported from ${BRANDS[batch.source.brand] ?? batch.source.brand} (${batch.source.name})`);
    for (const { bookmark, url } of supported) {
      let parentId = root;
      for (const part of bookmark.folder) parentId = await folder(parentId, part);
      const list = await getChildren(parentId);
      if (list.some(node => node.url === url)) continue;
      list.push(await chrome.bookmarks.create({ parentId, title: bookmark.title, url }));
      bookmarks++;
    }
  }
  const seen = new Set<string>();
  for (const visit of batch.history) {
    const url = new URL(visit.url).href;
    if (seen.has(url)) continue;
    seen.add(url);
    if ((await chrome.history.getVisits({ url })).length) continue;
    await chrome.history.addUrl({ url });
    history++;
  }
  for (const cookie of batch.cookies) {
    if (cookie.expirationDate != null && cookie.expirationDate <= Date.now() / 1000) continue;
    if (!await chrome.cookies.set(cookieDetails(cookie))) return { ok: false, reason: "apply_failed" };
    cookies++;
  }
  return { ok: true, id: batch.id, bookmarks, history, cookies, ...(skipped ? { skipped } : {}) };
}

function unhex(value: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(value.match(/../g) ?? [], pair => Number.parseInt(pair, 16));
}

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), byte => byte.toString(16).padStart(2, "0")).join("");
}

async function authorizedBatch(ticket: string, pair: ImportCredential): Promise<ImportDeliveryBatch> {
  const usedKey = `importTickets:${pair.device}`;
  const used = (await chrome.storage.session.get(usedKey))[usedKey] as string[] | undefined ?? [];
  if (used.includes(ticket)) throw new Error("Consumed import ticket");
  await chrome.storage.session.set({ [usedKey]: [...used, ticket] });
  const response = await fetch(`${pair.url}/v1/extension/import/${ticket}`, {
    headers: { Authorization: `Bearer ${pair.token}` }, credentials: "omit", redirect: "error", cache: "no-store", signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok) throw new Error("Import authorization unavailable");
  const signed: unknown = await response.json();
  if (!record(signed) || signed.ticket !== ticket || typeof signed.batch_id !== "string" || !/^[0-9a-f]{32}$/.test(signed.batch_id)
    || typeof signed.payload !== "string" || signed.payload.length > 2 * 1024 * 1024 || typeof signed.signature !== "string" || !/^[0-9a-f]{64}$/.test(signed.signature)) throw new Error("Invalid import authorization");
  const bytes = new TextEncoder().encode(signed.payload);
  const keyBytes = unhex(pair.secret);
  try {
    const payloadHash = hex(await crypto.subtle.digest("SHA-256", bytes));
    const key = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
    if (!await crypto.subtle.verify("HMAC", key, unhex(signed.signature), new TextEncoder().encode(ticket + signed.batch_id + payloadHash))) throw new Error("Invalid import authenticator");
    const batch: unknown = JSON.parse(signed.payload);
    if (!validBatch(batch) || batch.id !== signed.batch_id) throw new Error("Invalid authenticated import");
    return batch;
  } finally {
    bytes.fill(0); keyBytes.fill(0);
    signed.payload = "";
  }
}

function forgetCookieValues(batch: ImportDeliveryBatch): void {
  // JS strings and browser/HTTP internal buffers cannot be guaranteed erased.
  for (const cookie of batch.cookies) cookie.value = "";
}

export async function handleImportRequest(raw: unknown, senderOrigin: string | undefined): Promise<ImportReply | ImportPermissionReply | ImportStatusReply> {
  const origin = loopbackOrigin(senderOrigin);
  if (!origin) return { ok: false, reason: "refused" };
  const pair = await importCredential();
  if (!pair) return { ok: false, reason: "needs_pairing" };
  if (pair.url !== origin) return { ok: false, reason: "refused" };
  if (record(raw) && raw.type === IMPORT_STATUS) return { ok: true, device: pair.device };
  if (record(raw) && (raw.type === IMPORT_PERMISSION_OPEN || raw.type === IMPORT_APPLY)) {
    if (typeof raw.ticket !== "string" || !/^[0-9a-f]{64}$/.test(raw.ticket)) return { ok: false, reason: "invalid_batch" };
    const run = applying.then(async (): Promise<ImportReply | ImportPermissionReply> => {
      const batch = await authorizedBatch(raw.ticket as string, pair);
      try {
        if (raw.type === IMPORT_APPLY) return await applyBatch(batch);
        const origins = [...new Set(batch.cookies.map(cookie => {
          const url = new URL(cookie.url);
          return `${url.protocol}//${url.hostname}/*`;
        }))];
        const id = crypto.randomUUID();
        await chrome.storage.session.set({ [`importPermissions:${id}`]: { origin, origins, denied: false } });
        await chrome.tabs.create({ url: chrome.runtime.getURL(`import-permissions.html#${id}`) });
        return { ok: true, id, granted: await chrome.permissions.contains({ origins }), denied: false };
      } finally { forgetCookieValues(batch); }
    }).catch((): ImportReply => ({ ok: false, reason: "apply_failed" }));
    applying = run;
    return run;
  }
  if (record(raw) && raw.type === IMPORT_PERMISSION_STATUS) {
    if (typeof raw.id !== "string" || !/^[0-9a-f-]{36}$/.test(raw.id)) return { ok: false, reason: "invalid_batch" };
    const key = `importPermissions:${raw.id}`;
    const saved: unknown = (await chrome.storage.session.get(key))[key];
    if (!record(saved) || saved.origin !== origin || !Array.isArray(saved.origins) || !saved.origins.every(value => typeof value === "string")) return { ok: false, reason: "refused" };
    return { ok: true, id: raw.id, granted: await chrome.permissions.contains({ origins: saved.origins }), denied: saved.denied === true };
  }
  return { ok: false, reason: "invalid_batch" };
}
