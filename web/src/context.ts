import { createContext, useContext } from "react";
import type { Client } from "./api/client";

export const ClientContext = createContext<Client | null>(null);

export function useClient(): Client {
  const client = useContext(ClientContext);
  if (!client) throw new Error("useClient outside ClientContext");
  return client;
}

/**
 * Client-generated idempotency key for message submission. `crypto.randomUUID`
 * needs a secure context, which a plain-HTTP tailnet address is not.
 */
export function newMessageId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `web-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}
