import { describe, expect, it } from "vitest";
import type { RemoteStatus } from "../api/types";
import { offStatus, quickTunnel, tailscaleOn } from "./testDaemon";
import { connectionProblem, oneClickTransport, pairingOptions, recommendation, transportViews } from "./transports";

const view = (status: RemoteStatus, id: string) => transportViews(status).find((item) => item.id === id)!;

describe("transportViews", () => {
  it("shows every connection off on a fresh computer", () => {
    expect(transportViews(offStatus()).map((item) => [item.id, item.phase])).toEqual([
      ["tailscale", "off"],
      ["cloudflare", "off"],
      ["relay", "off"],
    ]);
  });

  it("tells a computer without Tailscale to install it before turning it on", () => {
    const status = offStatus({ tailscale: { installed: false, state: "unavailable", url: null, last_error: null } });
    expect(view(status, "tailscale").hint).toMatch(/Install the Tailscale app/);
  });

  it("maps Tailscale states to on, and to an error with a fix for a stopped app", () => {
    expect(view(tailscaleOn(), "tailscale")).toMatchObject({ phase: "on", url: "https://desk.tail1234.ts.net" });
    const stopped = tailscaleOn({ tailscale: { installed: true, state: "stopped", url: null, last_error: null } });
    expect(view(stopped, "tailscale")).toMatchObject({ phase: "error" });
    expect(view(stopped, "tailscale").hint).toMatch(/Open the Tailscale app/);
  });

  it("walks a Cloudflare tunnel through starting, on and failed", () => {
    const base = offStatus().configuration;
    const config = { ...base, cloudflare: { ...base.cloudflare, enabled: true } };
    const cloudflare = (state: string, url: string | null = null, last_error: string | null = null) =>
      offStatus({ configuration: config, cloudflare: { state, url, restarts: 0, last_error } });
    expect(view(offStatus({ configuration: config }), "cloudflare").phase).toBe("starting");
    expect(view(cloudflare("starting"), "cloudflare").phase).toBe("starting");
    expect(view(cloudflare("running", "https://x.trycloudflare.com"), "cloudflare")).toMatchObject({ phase: "on", url: "https://x.trycloudflare.com" });
    const failed = view(cloudflare("failed", null, "cloudflared not found"), "cloudflare");
    expect(failed.phase).toBe("error");
    expect(failed.hint).toMatch(/cloudflared not found.*installed/);
  });

  it("reports a relay that cannot be reached as an error that keeps retrying", () => {
    const base = offStatus().configuration;
    const config = { ...base, relay: { enabled: true, url: "https://relay.example.com", token_file: null } };
    const relay = (state: string, last_error: string | null = null) =>
      offStatus({ configuration: config, relay: { state, url: "https://relay.example.com", daemon_id: "id", daemon_key: "k", clients: 0, last_error, e2ee: true } });
    expect(view(relay("connected"), "relay")).toMatchObject({ phase: "on", url: "https://relay.example.com" });
    expect(view(relay("connecting"), "relay").phase).toBe("starting");
    const backoff = view(relay("backoff", "401 unauthorized"), "relay");
    expect(backoff.phase).toBe("error");
    expect(backoff.hint).toMatch(/401 unauthorized.*retrying/);
  });
});

describe("recommendation", () => {
  it("prefers Tailscale when it is installed and the relay otherwise", () => {
    expect(recommendation(offStatus()).id).toBe("tailscale");
    const none = offStatus({ tailscale: { installed: false, state: "unavailable", url: null, last_error: null } });
    expect(recommendation(none).id).toBe("relay");
  });
});

describe("pairingOptions", () => {
  it("offers only connections that are on, in the daemon's order", () => {
    expect(pairingOptions(offStatus())).toEqual([]);
    expect(pairingOptions(tailscaleOn())).toEqual([{ via: "tailscale", label: "Tailscale", url: "https://desk.tail1234.ts.net" }]);
  });

  it("falls back to a saved reverse-proxy address when no connection is managed", () => {
    expect(pairingOptions(offStatus({ public_url: "https://proxy.example.com" }))).toEqual([
      { via: null, label: "Saved address", url: "https://proxy.example.com" },
    ]);
  });
});

describe("connectionProblem", () => {
  it("says nothing is on for a fresh computer", () => {
    expect(connectionProblem(offStatus(), null)).toEqual({ kind: "none" });
  });

  it("words a tunnel's first start and its restarts differently", () => {
    expect(connectionProblem(quickTunnel({ state: "starting" }), null)).toMatchObject({ kind: "starting", message: expect.stringMatching(/^Starting Cloudflare Tunnel/) });
    expect(connectionProblem(quickTunnel({ state: "restarting", restarts: 1 }), null)).toEqual({
      kind: "starting",
      id: "cloudflare",
      message: "The tunnel isn't reachable yet. Retrying…",
    });
  });

  it("passes on the daemon's reason for a failed connection and prefers one that is still coming up", () => {
    expect(connectionProblem(quickTunnel({ state: "failed", last_error: "cloudflared not found" }), null)).toMatchObject({ kind: "error", id: "cloudflare" });
    const starting = quickTunnel({ state: "starting" });
    const both = { ...starting, tailscale: { installed: true, state: "stopped", url: null, last_error: null }, configuration: { ...starting.configuration, tailscale: { enabled: true, public_url: null } } };
    expect(connectionProblem(both, null).kind).toBe("starting");
  });

  it("reports a chosen connection that was turned off meanwhile", () => {
    expect(connectionProblem(offStatus(), "relay")).toEqual({ kind: "error", id: "relay", message: "Your own relay is turned off." });
  });
});

describe("oneClickTransport", () => {
  it("is Tailscale where installed, a saved relay next, otherwise a quick tunnel", () => {
    expect(oneClickTransport(offStatus())).toBe("tailscale");
    const missing = { installed: false, state: "unavailable", url: null, last_error: null };
    expect(oneClickTransport(offStatus({ tailscale: missing }))).toBe("cloudflare");
    const base = offStatus().configuration;
    expect(oneClickTransport(offStatus({ tailscale: missing, configuration: { ...base, relay: { enabled: false, url: "https://relay.example.com", token_file: null } } }))).toBe("relay");
  });
});
