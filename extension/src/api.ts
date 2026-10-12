import { createClient, type Client } from "../../web/src/api/client";
import { openSessionStream } from "../../web/src/api/sse";
import type { components } from "./schema";
import type { Pairing } from "./storage";
export { openSessionStream, type Client };
export type Session = components["schemas"]["Session"];
export type Ticket = components["schemas"]["Ticket"];
export type TabLedger = components["schemas"]["BrowserTabList"];
export function client(pair: Pairing, fetcher?: typeof fetch): Client { return createClient({ baseUrl: pair.url, token: () => pair.token, fetch: fetcher }); }
export async function ledger(pair: Pairing): Promise<TabLedger> {
  const response = await fetch(`${pair.url}/v1/browser/tabs`, { headers: { Authorization: `Bearer ${pair.token}` } });
  if (!response.ok) throw new Error(`Cannot read Agent Tabs (${response.status}).`);
  return await response.json() as TabLedger;
}
