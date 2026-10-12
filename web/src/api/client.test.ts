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

  it("keeps the daemon message for unknown codes and tolerates non-JSON bodies", async () => {
    const { client } = fakeClient(() => json(400, { code: "brand_new", message: "brand new failure" }));
    await expect(client.health()).rejects.toMatchObject({ code: "brand_new", message: "brand new failure" });
    const { client: plain } = fakeClient(() => new Response("Bad Gateway", { status: 502 }));
    await expect(plain.health()).rejects.toMatchObject({ status: 502, code: "http_502" });
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
});
