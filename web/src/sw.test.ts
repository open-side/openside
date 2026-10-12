import "fake-indexeddb/auto";
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { idbGet, SHARE_STORE } from "./pwa/idb";
import type { SharedItem } from "./pwa/share";

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

  it.each(["/theme-boot.js", "/manifest.webmanifest", "/icons/icon-192.png"])("serves %s network-first and from the cache when offline", async (path) => {
    const listeners: Record<string, Listener> = {};
    const scope = { location: { origin: "http://daemon.test" }, addEventListener: (type: string, l: Listener) => (listeners[type] = l) };
    const cached = new Response("cached boot");
    const match = vi.fn(async () => cached);
    const offline = vi.fn(async () => {
      throw new TypeError("offline");
    });
    new Function("self", "caches", "fetch", readFileSync("public/sw.js", "utf8"))(scope, { match, open: vi.fn() }, offline);
    const respondWith = vi.fn();
    const request = { url: `http://daemon.test${path}`, method: "GET", mode: "no-cors" } as Request;
    listeners.fetch!({ request, respondWith, waitUntil: vi.fn() });
    expect(offline).toHaveBeenCalledWith(request);
    await expect(respondWith.mock.calls[0]![0]).resolves.toBe(cached);
    expect(match).toHaveBeenCalledWith(path);
  });

  describe("share target", () => {
    interface Part {
      name: string;
      value: string;
      filename?: string;
      type?: string;
    }
    /**
     * A multipart body assembled by hand: under jsdom the global File/FormData are jsdom's, which
     * Node's Request cannot stream, while the worker only needs to parse what the browser sent.
     */
    function multipart(parts: Part[]): { body: Buffer; headers: Record<string, string> } {
      const boundary = "----openside-test";
      const chunks = parts.map((part) => {
        const disposition = `form-data; name="${part.name}"${part.filename === undefined ? "" : `; filename="${part.filename}"`}`;
        const type = part.type ? `\r\nContent-Type: ${part.type}` : "";
        return `--${boundary}\r\nContent-Disposition: ${disposition}${type}\r\n\r\n${part.value}\r\n`;
      });
      return { body: Buffer.from(`${chunks.join("")}--${boundary}--\r\n`), headers: { "content-type": `multipart/form-data; boundary=${boundary}` } };
    }

    async function share(parts: Part[], method = "POST", path = "/share-target"): Promise<{ respondWith: ReturnType<typeof vi.fn>; response?: Response }> {
      const listener = loadWorker();
      const respondWith = vi.fn();
      const request = new Request(`http://daemon.test${path}`, { method, ...(method === "POST" ? multipart(parts) : {}) });
      listener({ request, respondWith, waitUntil: vi.fn() });
      const answered = respondWith.mock.calls[0]?.[0] as Promise<Response> | undefined;
      return { respondWith, response: answered ? await answered : undefined };
    }

    it("parks the shared title, text, url and files in IndexedDB and redirects into the app", async () => {
      const { response } = await share([
        { name: "title", value: "Page title" },
        { name: "text", value: "look at this" },
        { name: "url", value: "https://example.com/a" },
        { name: "files", value: "png-bytes", filename: "shot.png", type: "image/png" },
        { name: "files", value: "", filename: "empty.png", type: "image/png" },
      ]);

      expect(response?.status).toBe(303);
      const location = new URL(response!.headers.get("location")!);
      expect(location.origin).toBe("http://daemon.test");
      expect(location.pathname).toBe("/");
      const id = location.searchParams.get("share")!;
      expect(id).toMatch(/^[A-Za-z0-9-]{8,64}$/);

      const stored = await idbGet<SharedItem>(SHARE_STORE, id);
      expect(stored).toMatchObject({ id, title: "Page title", text: "look at this", url: "https://example.com/a" });
      expect(stored!.files).toHaveLength(1);
      expect(stored!.files[0]!.name).toBe("shot.png");
      expect(stored!.files[0]!.type).toBe("image/png");
      expect(new TextDecoder().decode(stored!.files[0]!.data)).toBe("png-bytes");
      expect(Date.now() - stored!.received_at).toBeLessThan(5000);
    });

    it("accepts a text-only share", async () => {
      const { response } = await share([{ name: "text", value: "https://example.com/only-a-link" }]);
      const id = new URL(response!.headers.get("location")!).searchParams.get("share")!;
      expect(await idbGet<SharedItem>(SHARE_STORE, id)).toMatchObject({ text: "https://example.com/only-a-link", title: "", url: "", files: [] });
    });

    it("keeps at most four files", async () => {
      const files = Array.from({ length: 6 }, (_, i) => ({ name: "files", value: `f${i}`, filename: `f${i}.png`, type: "image/png" }));
      const { response } = await share(files);
      const id = new URL(response!.headers.get("location")!).searchParams.get("share")!;
      expect((await idbGet<SharedItem>(SHARE_STORE, id))!.files).toHaveLength(4);
    });

    it("lands on a visible failure instead of an error page when the body cannot be read", async () => {
      const listener = loadWorker();
      const respondWith = vi.fn();
      const request = new Request("http://daemon.test/share-target", { method: "POST", body: "not multipart", headers: { "content-type": "text/plain" } });
      listener({ request, respondWith, waitUntil: vi.fn() });
      const response = (await respondWith.mock.calls[0]![0]) as Response;
      expect(response.status).toBe(303);
      expect(response.headers.get("location")).toBe("http://daemon.test/?share=failed");
    });

    it("only claims POST /share-target", async () => {
      const get = await share([], "GET");
      expect(get.respondWith).not.toHaveBeenCalled();
      const other = await share([], "POST", "/v1/share-target");
      expect(other.respondWith).not.toHaveBeenCalled();
    });
  });

  describe("badge", () => {
    type Handler = (event: { waitUntil: (p: Promise<unknown>) => void; notification?: unknown; data?: unknown }) => void;
    function workerWith(shown: { data: unknown }[]) {
      const handlers: Record<string, Handler> = {};
      const setAppBadge = vi.fn(async () => undefined);
      const clearAppBadge = vi.fn(async () => undefined);
      const scope = {
        location: { origin: "http://daemon.test" },
        navigator: { setAppBadge, clearAppBadge },
        registration: { showNotification: vi.fn(async () => undefined), getNotifications: async () => shown },
        clients: { matchAll: async () => [], openWindow: async () => null },
        addEventListener: (type: string, handler: Handler) => (handlers[type] = handler),
      };
      new Function("self", "caches", "fetch", readFileSync("public/sw.js", "utf8"))(scope, {}, vi.fn());
      return { handlers, setAppBadge, clearAppBadge };
    }
    async function run(handler: Handler, event: Record<string, unknown> = {}) {
      const waits: Promise<unknown>[] = [];
      handler({ ...event, waitUntil: (p) => waits.push(p) });
      await Promise.all(waits);
    }

    it("counts the ticket notifications on screen after a push", async () => {
      const { handlers, setAppBadge } = workerWith([{ data: { kind: "ticket" } }, { data: { kind: "settled" } }, { data: { kind: "ticket" } }]);
      await run(handlers.push!, { data: { json: () => ({ kind: "ticket", title: "Approve?", ticket_id: "t1", url: "/sessions/s1" }) } });
      expect(setAppBadge).toHaveBeenCalledWith(2);
    });

    it("clears the badge once no ticket notification is left", async () => {
      const { handlers, clearAppBadge, setAppBadge } = workerWith([{ data: { kind: "settled" } }]);
      await run(handlers.notificationclose!);
      expect(clearAppBadge).toHaveBeenCalled();
      expect(setAppBadge).not.toHaveBeenCalled();
    });
  });
});
