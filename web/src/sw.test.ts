import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

type Listener = (event: { request: Request; respondWith: (r: unknown) => void; waitUntil: (p: unknown) => void }) => void;

/** Load sw.js against a stub worker scope and return its fetch listener. */
function loadWorker(): Listener {
  const listeners: Record<string, Listener> = {};
  const scope = {
    location: { origin: "http://daemon.test" },
    addEventListener: (type: string, listener: Listener) => {
      listeners[type] = listener;
    },
  };
  // Vitest runs from the web package root; the worker is shipped unbundled from public/.
  const source = readFileSync("public/sw.js", "utf8");
  const pending = () => Promise.withResolvers<never>().promise;
  new Function("self", "caches", "fetch", source)(scope, { match: pending, open: pending }, pending);
  return listeners.fetch!;
}

function intercepted(listener: Listener, path: string, mode: RequestMode = "navigate"): boolean {
  const respondWith = vi.fn();
  const request = { url: `http://daemon.test${path}`, method: "GET", mode } as Request;
  listener({ request, respondWith, waitUntil: vi.fn() });
  return respondWith.mock.calls.length > 0;
}

describe("service worker", () => {
  it("never intercepts the API, including the bare /v1 path", () => {
    const listener = loadWorker();
    for (const path of ["/v1", "/v1/", "/v1/sessions", "/v1/sessions/s1/events"]) {
      expect(intercepted(listener, path), path).toBe(false);
      expect(intercepted(listener, path, "cors"), path).toBe(false);
    }
  });

  it("serves the offline shell for navigations and caches hashed assets", () => {
    const listener = loadWorker();
    expect(intercepted(listener, "/")).toBe(true);
    expect(intercepted(listener, "/sessions/s1")).toBe(true);
    expect(intercepted(listener, "/v1x")).toBe(true);
    expect(intercepted(listener, "/assets/index-abc.js", "cors")).toBe(true);
    expect(intercepted(listener, "/unknown.json", "cors")).toBe(false);
  });

  it("serves theme-boot.js network-first and from the cache when offline", async () => {
    const listeners: Record<string, Listener> = {};
    const scope = { location: { origin: "http://daemon.test" }, addEventListener: (type: string, l: Listener) => (listeners[type] = l) };
    const cached = new Response("cached boot");
    const match = vi.fn(async () => cached);
    const offline = vi.fn(async () => {
      throw new TypeError("offline");
    });
    new Function("self", "caches", "fetch", readFileSync("public/sw.js", "utf8"))(scope, { match, open: vi.fn() }, offline);
    const respondWith = vi.fn();
    const request = { url: "http://daemon.test/theme-boot.js", method: "GET", mode: "no-cors" } as Request;
    listeners.fetch!({ request, respondWith, waitUntil: vi.fn() });
    expect(offline).toHaveBeenCalledWith(request);
    await expect(respondWith.mock.calls[0]![0]).resolves.toBe(cached);
    expect(match).toHaveBeenCalledWith("/theme-boot.js");
  });
});
