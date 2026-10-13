import type { Client } from "../api/client";
import type { CreatePairingRequest, Device, PairingOffer, RemoteStatus } from "../api/types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";

/** A computer with nothing turned on, asked from its own browser (local proof accepted). */
export function offStatus(patch: Partial<RemoteStatus> = {}): RemoteStatus {
  return {
    modes: [],
    public_url: null,
    cloudflare: null,
    relay: null,
    tailscale: { installed: true, state: "stopped", url: null, last_error: null },
    configuration: {
      tailscale: { enabled: false, public_url: null },
      cloudflare: { enabled: false, tunnel: "quick", hostname: null, token_file: null },
      relay: { enabled: false, url: null, token_file: null },
    },
    pairing_allowed: true,
    configure_allowed: true,
    restart_required: false,
    ...patch,
  } as RemoteStatus;
}

export function tailscaleOn(patch: Partial<RemoteStatus> = {}): RemoteStatus {
  return offStatus({
    modes: ["tailscale"],
    public_url: "https://desk.tail1234.ts.net",
    tailscale: { installed: true, state: "running", url: "https://desk.tail1234.ts.net", last_error: null },
    configuration: {
      tailscale: { enabled: true, public_url: "https://desk.tail1234.ts.net" },
      cloudflare: { enabled: false, tunnel: "quick", hostname: null, token_file: null },
      relay: { enabled: false, url: null, token_file: null },
    },
    ...patch,
  } as Partial<RemoteStatus>);
}

export function offer(patch: Partial<PairingOffer> = {}): PairingOffer {
  return {
    code: "K7QM2XPD",
    path: "/pair",
    url: "https://desk.tail1234.ts.net/pair#K7QM2XPD",
    scopes: ["read"],
    expires_at: new Date(Date.now() + 180_000).toISOString(),
    transport: "tailscale",
    kind: "person_device",
    ...patch,
  };
}

export interface RemoteDaemonState {
  status: RemoteStatus;
  devices: Device[];
  /** Answer for the next pairing request; defaults to a fresh offer. */
  offer?: () => PairingOffer;
  /** Overrides any route's answer; return undefined to fall through. */
  override?: (call: Call) => Response | undefined;
}

export interface RemoteDaemon {
  client: Client;
  calls: Call[];
  state: RemoteDaemonState;
}

/** A daemon with the remote-access routes; `state` is read on every request so tests can mutate it. */
export function remoteDaemon(patch: Partial<RemoteDaemonState> = {}): RemoteDaemon {
  const state: RemoteDaemonState = {
    status: offStatus(),
    devices: [{ device_id: "d1", name: "This laptop", kind: "person_device", scopes: ["admin"], created_at: "2026-10-01T00:00:00Z" }],
    ...patch,
  };
  const fake = fakeClient((call) => {
    const forced = state.override?.(call);
    if (forced) return forced;
    const path = new URL(call.path, "http://daemon.test").pathname;
    const key = `${call.method} ${path}`;
    if (key === "GET /v1/remote/status") return json(200, state.status);
    if (key === "GET /v1/devices") return json(200, { devices: state.devices });
    if (key === "POST /v1/pairing") return json(200, (state.offer ?? offer)());
    return json(404, { code: "not_found", message: "nope" });
  });
  return { ...fake, state };
}

/** Bodies of the pairing requests a fake daemon received, oldest first. */
export function pairingRequests(calls: Call[]): CreatePairingRequest[] {
  // The fake records parsed JSON bodies; only the pairing route's body has this shape.
  const bodies: unknown[] = calls.filter((call) => call.method === "POST" && call.path === "/v1/pairing").map((call) => call.body);
  return bodies as CreatePairingRequest[];
}
