import type { RemoteStatus } from "../api/types";

export type TransportId = "tailscale" | "cloudflare" | "relay";

/** `starting` covers both a connection coming up and one that is saved but not running yet. */
export type TransportPhase = "off" | "starting" | "on" | "error";

export interface TransportView {
  id: TransportId;
  phase: TransportPhase;
  /** The address a phone opens, once the connection is on. */
  url: string | null;
  /** What is wrong and how to fix it (error), or what is pending (starting). */
  hint: string | null;
}

export const TRANSPORT_NAMES: Record<TransportId, string> = {
  tailscale: "Tailscale",
  cloudflare: "Cloudflare Tunnel",
  relay: "Your own relay",
};

/** Daemon errors arrive without a final full stop; the fix hint after them needs one. */
const sentence = (text: string) => (/[.!?]$/.test(text) ? text : `${text}.`);

function tailscale(status: RemoteStatus): TransportView {
  const { installed, state, url, last_error } = status.tailscale;
  const enabled = status.configuration.tailscale.enabled;
  const configured = status.configuration.tailscale.public_url;
  if (!enabled) {
    return { id: "tailscale", phase: "off", url: null, hint: installed ? null : "Install the Tailscale app on this computer and sign in first." };
  }
  if (state === "running") return { id: "tailscale", phase: "on", url: url ?? configured ?? null, hint: null };
  if (state === "stopped") {
    return { id: "tailscale", phase: "error", url: null, hint: "Tailscale is installed but not running. Open the Tailscale app, sign in, then try again." };
  }
  if (!installed) return { id: "tailscale", phase: "error", url: null, hint: "Tailscale was not found on this computer. Install it and sign in, then try again." };
  return { id: "tailscale", phase: "error", url: null, hint: last_error ?? "Tailscale could not share openside. Check that HTTPS is enabled for your tailnet." };
}

function cloudflare(status: RemoteStatus): TransportView {
  const runtime = status.cloudflare;
  if (!status.configuration.cloudflare.enabled) return { id: "cloudflare", phase: "off", url: null, hint: null };
  if (!runtime) return { id: "cloudflare", phase: "starting", url: null, hint: null };
  if (runtime.state === "running" && runtime.url) return { id: "cloudflare", phase: "on", url: runtime.url ?? null, hint: null };
  if (runtime.state === "failed") {
    return {
      id: "cloudflare",
      phase: "error",
      url: null,
      hint: `${sentence(runtime.last_error ?? "The tunnel could not start")} Check that cloudflared is installed on this computer, then turn it off and on again.`,
    };
  }
  return { id: "cloudflare", phase: "starting", url: null, hint: runtime.last_error ?? null };
}

function relay(status: RemoteStatus): TransportView {
  const runtime = status.relay;
  if (!status.configuration.relay.enabled) return { id: "relay", phase: "off", url: null, hint: null };
  if (!runtime) return { id: "relay", phase: "starting", url: null, hint: null };
  if (runtime.state === "connected") return { id: "relay", phase: "on", url: runtime.url ?? null, hint: null };
  if (runtime.state === "backoff") {
    return {
      id: "relay",
      phase: "error",
      url: null,
      hint: `${sentence(runtime.last_error ?? "The relay did not answer")} openside keeps retrying; check the address and the token file.`,
    };
  }
  return { id: "relay", phase: "starting", url: null, hint: null };
}

export function transportViews(status: RemoteStatus): TransportView[] {
  return [tailscale(status), cloudflare(status), relay(status)];
}

/**
 * One suggestion so a first-time user is not left to compare three options. Tailscale wins when
 * it is already on this computer. Otherwise the relay: it opens nothing on the network, while a
 * tunnel needs a Cloudflare account and, for everyday use, a domain.
 */
export function recommendation(status: RemoteStatus): { id: TransportId; reason: string } {
  if (status.tailscale.installed) {
    return { id: "tailscale", reason: "Tailscale is already on this computer. Only your own devices can reach openside, and nothing is opened to the internet." };
  }
  return { id: "relay", reason: "Nothing on your network is opened and traffic stays encrypted. You need a relay server of your own." };
}

export interface PairingOption {
  /** `via` for the pairing request; `null` lets the daemon use the plain `remote.public_url`. */
  via: TransportId | null;
  label: string;
  url: string;
}

/**
 * Connections a phone can be paired through right now, preferred first (the daemon's own order
 * when it lists one). A saved reverse-proxy address counts when no managed connection is on.
 */
export function pairingOptions(status: RemoteStatus): PairingOption[] {
  const on = transportViews(status).filter((view) => view.phase === "on" && view.url !== null);
  const rank = (id: TransportId) => {
    const index = status.modes.indexOf(id);
    return index === -1 ? status.modes.length : index;
  };
  const options = on
    .sort((a, b) => rank(a.id) - rank(b.id))
    .map((view): PairingOption => ({ via: view.id, label: TRANSPORT_NAMES[view.id], url: view.url! }));
  if (options.length === 0 && status.public_url) options.push({ via: null, label: "Saved address", url: status.public_url });
  return options;
}
