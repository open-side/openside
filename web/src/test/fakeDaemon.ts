import { vi } from "vitest";
import { createClient, type Client } from "../api/client";

export interface Call {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
}

type Handler = (call: Call) => Response | Promise<Response>;

export function json(status: number, body: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Records every request and answers through `handler`; no network involved. */
export function fakeClient(handler: Handler, token = "test-token"): { client: Client; calls: Call[] } {
  const calls: Call[] = [];
  const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "http://daemon.test");
    const call: Call = {
      method: init?.method ?? "GET",
      path: url.pathname + url.search,
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    return handler(call);
  });
  return { client: createClient({ token: () => token, fetch: fetchImpl as typeof fetch }), calls };
}
