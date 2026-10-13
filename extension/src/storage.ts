/** `device` is set only when the extension minted its token itself from a web pairing code. */
export interface Pairing { url: string; token: string; device?: string }
export function validateDaemonUrl(value: string): string {
  const url = new URL(value);
  if (url.username || url.password || url.search || url.hash || (url.pathname !== "/" && url.pathname !== "")) throw new Error("Enter a daemon origin without a path or credentials.");
  if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))) throw new Error("Use HTTPS, or HTTP on loopback only.");
  return url.origin;
}
/** A direct pairing link is `<daemon origin>/pair#<code>`; relay links carry key material this page cannot use. */
export function parsePairingLink(value: string): { url: string; code: string } {
  const link = new URL(value.trim());
  const code = decodeURIComponent(link.hash.slice(1)).trim();
  if (link.pathname !== "/pair" || !code || code.includes("=")) throw new Error("That is not a direct openside pairing link. Copy it from Connect a phone, using a Tailscale, Cloudflare or local address.");
  return { url: validateDaemonUrl(link.origin), code };
}
export async function pairing(): Promise<Pairing> {
  const stored = await chrome.storage.session.get("pairing");
  if (!stored.pairing) throw new Error("Pair openside in the extension options first.");
  return stored.pairing as Pairing;
}
/** Survives a browser restart (local storage) while the token it belongs to does not (session storage). */
export const IMPORT_PAIRING = "importPairing";
export interface ImportPairing { device: string; secret: string; origin: string }
export type ImportCredential = Pairing & ImportPairing;
export async function retainedImportPairing(origin: string): Promise<ImportPairing | null> {
  const kept = (await chrome.storage.local.get(IMPORT_PAIRING))[IMPORT_PAIRING] as Partial<ImportPairing> | undefined;
  if (!kept || kept.origin !== origin || typeof kept.device !== "string" || !/^[0-9a-f]{32}$/.test(kept.device) || typeof kept.secret !== "string" || !/^[0-9a-f]{64}$/.test(kept.secret)) return null;
  return { device: kept.device, secret: kept.secret, origin };
}
/** The daemon's import secret is 32 random bytes as 64 lowercase hex characters. */
export function importSecretOf(redeemed: object): string | null {
  return "import_secret" in redeemed && typeof redeemed.import_secret === "string" && /^[0-9a-f]{64}$/.test(redeemed.import_secret) ? redeemed.import_secret : null;
}
/** Token, daemon URL and secret of the import pairing, or null when any part is missing, stale (token replaced or lost with the browser session) or malformed. */
export async function importCredential(): Promise<ImportCredential | null> {
  const current = (await chrome.storage.session.get("pairing")).pairing as Pairing | undefined;
  const kept = (await chrome.storage.local.get(IMPORT_PAIRING))[IMPORT_PAIRING] as Partial<ImportPairing> | undefined;
  if (!current?.device || !kept || kept.device !== current.device || kept.origin !== current.url || typeof kept.secret !== "string" || !/^[0-9a-f]{64}$/.test(kept.secret)) return null;
  return { ...current, device: current.device, secret: kept.secret, origin: current.url };
}
export async function tabSessions(tabId: number): Promise<string[]> {
  const key = `sessions:${tabId}`;
  return (await chrome.storage.session.get(key))[key] as string[] ?? [];
}
export async function rememberSession(tabId: number, id: string): Promise<void> {
  await chrome.storage.session.set({ [`sessions:${tabId}`]: [...new Set([id, ...await tabSessions(tabId)])] });
}
