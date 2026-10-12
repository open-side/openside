export interface Pairing { url: string; token: string }
export function validateDaemonUrl(value: string): string {
  const url = new URL(value);
  if (url.username || url.password || url.search || url.hash || (url.pathname !== "/" && url.pathname !== "")) throw new Error("Enter a daemon origin without a path or credentials.");
  if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))) throw new Error("Use HTTPS, or HTTP on loopback only.");
  return url.origin;
}
export async function pairing(): Promise<Pairing> {
  const stored = await chrome.storage.session.get("pairing");
  if (!stored.pairing) throw new Error("Pair openside in the extension options first.");
  return stored.pairing as Pairing;
}
export async function tabSessions(tabId: number): Promise<string[]> {
  const key = `sessions:${tabId}`;
  return (await chrome.storage.session.get(key))[key] as string[] ?? [];
}
export async function rememberSession(tabId: number, id: string): Promise<void> {
  await chrome.storage.session.set({ [`sessions:${tabId}`]: [...new Set([id, ...await tabSessions(tabId)])] });
}
