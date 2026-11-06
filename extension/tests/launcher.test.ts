import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../../web/src/api/client";

const api = vi.hoisted(() => ({
  exchangePairing: vi.fn(async (_code: string) => ({ token: "launcher-token", device_id: "dev-launcher" })),
  origins: [] as string[],
}));
vi.mock("../src/api", () => ({ client: (pair: { url: string }) => { api.origins.push(pair.url); return api; } }));

import { adoptLauncherPairing, forgetRevokedLauncherPairing, LAUNCH_FILE } from "../src/launcher";

const session: Record<string, unknown> = {};
vi.stubGlobal("chrome", {
  storage: { session: {
    get: vi.fn(async (key: string) => (key in session ? { [key]: session[key] } : {})),
    set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(session, items); }),
    remove: vi.fn(async (key: string) => { delete session[key]; }),
  } },
  runtime: { getURL: (path: string) => `chrome-extension://fixture/${path}`, getManifest: () => ({ externally_connectable: { matches: [`${UI}/*`] } }) },
});
const UI = "http://127.0.0.1:47820";
const CODE = "c".repeat(64);
let file: unknown;
const fetchFile = vi.spyOn(globalThis, "fetch");

beforeEach(() => {
  vi.clearAllMocks();
  api.origins.length = 0;
  for (const key of Object.keys(session)) delete session[key];
  file = { url: UI, code: CODE };
  fetchFile.mockImplementation(async () => file === null ? Promise.reject(new TypeError("net::ERR_FILE_NOT_FOUND")) : new Response(JSON.stringify(file)));
});

describe("adoptLauncherPairing", () => {
  it("redeems the launcher's code against the loopback daemon and stores a launcher pairing", async () => {
    expect(await adoptLauncherPairing()).toBe(true);
    expect(fetchFile).toHaveBeenCalledWith(`chrome-extension://fixture/${LAUNCH_FILE}`, { cache: "no-store" });
    expect(api.exchangePairing).toHaveBeenCalledWith(CODE);
    expect(api.origins).toEqual([UI]);
    expect(session.pairing).toEqual({ url: UI, token: "launcher-token", device: "dev-launcher", launcher: true });
    // The stored digest is not the code itself.
    expect(JSON.stringify(session)).not.toContain(CODE);
  });

  it("concurrent callers share one exchange", async () => {
    expect(await Promise.all([adoptLauncherPairing(), adoptLauncherPairing(), adoptLauncherPairing()])).toEqual([true, true, true]);
    expect(api.exchangePairing).toHaveBeenCalledTimes(1);
  });

  it("leaves an existing pairing alone and never reads the file", async () => {
    session.pairing = { url: "https://daemon.example", token: "pasted" };
    expect(await adoptLauncherPairing()).toBe(true);
    expect(fetchFile).not.toHaveBeenCalled();
    expect(session.pairing).toEqual({ url: "https://daemon.example", token: "pasted" });
  });

  it("keeps a pairing that landed while the code was being redeemed", async () => {
    api.exchangePairing.mockImplementationOnce(async () => {
      session.pairing = { url: UI, token: "from-the-web-app", device: "dev-web" };
      return { token: "launcher-token", device_id: "dev-launcher" };
    });
    expect(await adoptLauncherPairing()).toBe(true);
    expect(session.pairing).toEqual({ url: UI, token: "from-the-web-app", device: "dev-web" });
  });

  it("does nothing without a launch file", async () => {
    file = null;
    expect(await adoptLauncherPairing()).toBe(false);
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(session.pairing).toBeUndefined();
  });

  it("refuses anything but the daemon origin the manifest names and a well-formed code", async () => {
    for (const bad of [
      { url: "https://daemon.example", code: CODE },
      { url: "http://192.0.2.1:47820", code: CODE },
      // Another loopback port or host spelling is not the daemon this extension trusts.
      { url: "http://127.0.0.1:47821", code: CODE },
      { url: "http://localhost:47820", code: CODE },
      { url: "http://[::1]:47820", code: CODE },
      { url: `${UI}/path`, code: CODE },
      { url: `http://user@127.0.0.1:47820`, code: CODE },
      { url: UI, code: "short" },
      { url: UI, code: CODE.toUpperCase() },
      { url: UI },
      "not an object",
    ]) {
      file = bad;
      expect(await adoptLauncherPairing()).toBe(false);
    }
    expect(api.exchangePairing).not.toHaveBeenCalled();
  });

  it("tries a code once: a refused or spent code is not retried as a failed guess", async () => {
    api.exchangePairing.mockRejectedValueOnce(new ApiError(410, "pairing_expired"));
    expect(await adoptLauncherPairing()).toBe(false);
    expect(await adoptLauncherPairing()).toBe(false);
    expect(api.exchangePairing).toHaveBeenCalledTimes(1);
    // The next launch writes a new code.
    file = { url: UI, code: "d".repeat(64) };
    expect(await adoptLauncherPairing()).toBe(true);
    expect(api.exchangePairing).toHaveBeenCalledTimes(2);
  });
});

describe("forgetRevokedLauncherPairing", () => {
  it("drops only a launcher pairing, and only when the daemon no longer knows its token", async () => {
    session.pairing = { url: UI, token: "launcher-token", device: "dev-launcher", launcher: true };
    expect(await forgetRevokedLauncherPairing(new ApiError(503, "unavailable"))).toBe(false);
    expect(await forgetRevokedLauncherPairing(new Error("offline"))).toBe(false);
    expect(session.pairing).toBeDefined();
    expect(await forgetRevokedLauncherPairing(new ApiError(401, "unauthorized"))).toBe(true);
    expect(session.pairing).toBeUndefined();
    session.pairing = { url: UI, token: "pasted" };
    expect(await forgetRevokedLauncherPairing(new ApiError(401, "unauthorized"))).toBe(false);
    expect(session.pairing).toEqual({ url: UI, token: "pasted" });
  });
});
