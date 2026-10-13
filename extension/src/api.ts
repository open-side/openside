import { createClient, type Client } from "../../web/src/api/client";
import { openSessionStream } from "../../web/src/api/sse";
import type { components } from "./schema";
import type { ImportPairing, Pairing } from "./storage";
export { openSessionStream, type Client };
export type Session = components["schemas"]["Session"];
export type Ticket = components["schemas"]["Ticket"];
export type TabLedger = components["schemas"]["BrowserTabList"];
export function client(pair: Pairing, fetcher?: typeof fetch): Client { return createClient({ baseUrl: pair.url, token: () => pair.token, fetch: fetcher }); }

export async function exchangeImportPairing(origin: string, code: string, previous: ImportPairing | null): Promise<{ paired: components["schemas"]["PairDeviceResponse"]; continuous: boolean }> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  let nonce = "";
  if (previous) {
    nonce = Array.from(crypto.getRandomValues(new Uint8Array(32)), byte => byte.toString(16).padStart(2, "0")).join("");
    headers["x-openside-import-device"] = previous.device;
    headers["x-openside-import-nonce"] = nonce;
  }
  const response = await fetch(`${origin}/v1/pairing/exchange`, { method: "POST", headers, body: JSON.stringify({ code }), credentials: "omit", redirect: "error", cache: "no-store", signal: AbortSignal.timeout(15_000) });
  if (!response.ok) throw new Error("Pairing exchange refused");
  const payload = await response.text();
  const paired = JSON.parse(payload) as components["schemas"]["PairDeviceResponse"];
  const proof = response.headers.get("x-openside-import-proof");
  let continuous = false;
  if (previous && proof && /^[0-9a-f]{64}$/.test(proof)) {
    const bytes = new TextEncoder().encode(payload);
    const keyBytes = Uint8Array.from(previous.secret.match(/../g) ?? [], pair => Number.parseInt(pair, 16));
    try {
      const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
      const prefix = new TextEncoder().encode(`openside-import-pairing-v1\n${previous.device}${nonce}`);
      const signed = new Uint8Array(prefix.length + hash.length);
      signed.set(prefix); signed.set(hash, prefix.length);
      const key = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
      // A nonce-only proof could be transplanted onto an attacker's replacement credential.
      continuous = await crypto.subtle.verify("HMAC", key, Uint8Array.from(proof.match(/../g) ?? [], pair => Number.parseInt(pair, 16)), signed);
    } catch { continuous = false; }
    finally { bytes.fill(0); keyBytes.fill(0); }
  }
  return { paired, continuous };
}
export async function ledger(pair: Pairing): Promise<TabLedger> {
  const response = await fetch(`${pair.url}/v1/browser/tabs`, { headers: { Authorization: `Bearer ${pair.token}` } });
  if (!response.ok) throw new Error(`Cannot read Agent Tabs (${response.status}).`);
  return await response.json() as TabLedger;
}
