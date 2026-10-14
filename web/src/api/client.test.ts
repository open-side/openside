import { describe, expect, it, vi } from "vitest";
import { ApiError, createClient } from "./client";
import { fakeClient, json } from "../test/fakeDaemon";

describe("API client", () => {
  it("maps daemon error envelopes to ApiError with a readable message", async () => {
    const { client } = fakeClient(() => json(409, { code: "invalid_transition", message: "invalid transition" }));
    const error = await client.resolveTicket("t1", true).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 409, code: "invalid_transition" });
    expect((error as ApiError).message).toMatch(/no longer possible/);
  });

  it("tells the person where to fix an unavailable model account when a send is refused", async () => {
    const { client } = fakeClient(() => json(409, { code: "auth_unavailable", message: "auth unavailable" }));
    const error = await client.resolveTicket("t1", true).catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 409, code: "auth_unavailable" });
    expect((error as ApiError).message).toMatch(/Models & accounts/);
  });

  it("keeps the daemon message for unknown codes and tolerates non-JSON bodies", async () => {
    const { client } = fakeClient(() => json(400, { code: "brand_new", message: "brand new failure" }));
    await expect(client.health()).rejects.toMatchObject({ code: "brand_new", message: "brand new failure" });
    const { client: plain } = fakeClient(() => new Response("Bad Gateway", { status: 502 }));
    await expect(plain.health()).rejects.toMatchObject({ status: 502, code: "http_502" });
  });

  it("sends the local-access secret only on the remote-access calls, and never through a relay", async () => {
    const seen: Record<string, string>[] = [];
    const fetchImpl = (async (_url: RequestInfo | URL, init?: RequestInit) => {
      seen.push((init?.headers ?? {}) as Record<string, string>);
      return json(200, {});
    }) as typeof fetch;
    const client = createClient({ token: () => "t", fetch: fetchImpl, localSecret: () => "per-boot" });
    await client.remoteStatus();
    await client.setTailscale(true);
    await client.createPairing({ name: "p", scopes: ["read"], kind: "person_device", via: null });
    await client.health();
    expect(seen.map((headers) => headers["X-Openside-Local-Secret"])).toEqual(["per-boot", "per-boot", "per-boot", undefined]);

    seen.length = 0;
    const viaRelay = createClient({ token: () => "t", fetch: fetchImpl, relay: true, localSecret: () => "per-boot" });
    await viaRelay.remoteStatus();
    expect(seen[0]?.["X-Openside-Local-Secret"]).toBeUndefined();
  });

  it("sends the local-access secret only when approving an access request, so a remote admin cannot", async () => {
    const seen: { method: string; url: string; secret?: string }[] = [];
    const fetchImpl = (async (url: RequestInfo | URL, init?: RequestInit) => {
      seen.push({ method: init?.method ?? "GET", url: String(url), secret: (init?.headers as Record<string, string>)["X-Openside-Local-Secret"] });
      return json(200, {});
    }) as typeof fetch;
    const client = createClient({ token: () => "t", fetch: fetchImpl, localSecret: () => "per-boot" });
    await client.myDevice();
    await client.requestAccess(["admin"]);
    await client.accessRequests();
    await client.approveAccessRequest("r 1");
    await client.denyAccessRequest("r1");
    expect(seen.map(({ method, url, secret }) => `${method} ${url.replace(/^https?:\/\/[^/]+/, "")} ${secret ?? "-"}`)).toEqual([
      "GET /v1/devices/me -",
      "POST /v1/devices/me/access-requests -",
      "GET /v1/access-requests -",
      "POST /v1/access-requests/r%201/approve per-boot",
      "POST /v1/access-requests/r1/deny -",
    ]);
  });

  it("sends the local-access secret on the operator writes and not on their reads", async () => {
    const seen: string[] = [];
    const fetchImpl = (async (url: RequestInfo | URL, init?: RequestInit) => {
      const secret = (init?.headers as Record<string, string>)["X-Openside-Local-Secret"];
      seen.push(`${init?.method ?? "GET"} ${String(url).replace(/^https?:\/\/[^/]+/, "")} ${secret ?? "-"}`);
      return json(200, {});
    }) as typeof fetch;
    const client = createClient({ token: () => "t", fetch: fetchImpl, localSecret: () => "per-boot" });
    await client.daemonStatus();
    await client.restartDaemon();
    await client.downloadMemoryModel();
    await client.setSyncConfig({ settings: true, memory_tags: [] });
    await client.runSync();
    expect(seen).toEqual([
      "GET /v1/daemon -",
      "POST /v1/daemon/restart per-boot",
      "POST /v1/memory/model/download per-boot",
      "PUT /v1/sync/config per-boot",
      "POST /v1/sync/run -",
    ]);
  });

  it("reports network failures and 401s distinctly", async () => {
    const unauthorized = vi.fn();
    const offline = createClient({
      token: () => "t",
      fetch: (() => Promise.reject(new TypeError("Failed to fetch"))) as typeof fetch,
    });
    await expect(offline.health()).rejects.toMatchObject({ status: 0, code: "network_error" });
    const rejected = createClient({
      token: () => "t",
      onUnauthorized: unauthorized,
      fetch: (async () => json(401, { code: "unauthorized", message: "unauthorized" })) as typeof fetch,
    });
    await expect(rejected.listSessions()).rejects.toMatchObject({ code: "unauthorized" });
    expect(unauthorized).toHaveBeenCalledOnce();
  });

  it("sends the bearer token, JSON bodies and encoded query parameters", async () => {
    const { client, calls } = fakeClient((call) =>
      call.method === "GET" ? json(200, { sessions: [] }) : json(200, { id: "a/b" }),
    );
    await client.listSessions({ q: " report & co ", archived: true });
    await client.updateSession("a/b", { pinned: true });
    expect(calls[0]!.path).toBe("/v1/sessions?q=report+%26+co&archived=true");
    expect(calls[0]!.headers.Authorization).toBe("Bearer test-token");
    expect(calls[1]).toMatchObject({ method: "PATCH", path: "/v1/sessions/a%2Fb", body: { pinned: true } });
    expect(calls[1]!.headers["Content-Type"]).toBe("application/json");
  });
  it("accepts empty 202 prewarm responses without decoding JSON", async () => {
    const { client, calls } = fakeClient(() => new Response(null, { status: 202 }));
    await expect(client.prewarmSession("a/b")).resolves.toBeUndefined();
    expect(calls.map((call) => call.path)).toEqual(["/v1/sessions/a%2Fb/prewarm"]);
    expect(calls.every((call) => call.headers.Authorization === "Bearer test-token")).toBe(true);
  });
});
