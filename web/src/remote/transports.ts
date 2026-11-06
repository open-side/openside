import type { Client } from "../api/client";
import type { RemoteStatus } from "../api/types";
import { t, type MessageKey } from "../i18n";

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

const NAME_KEYS: Record<TransportId, MessageKey> = {
  tailscale: "connections.transport.name.tailscale",
  cloudflare: "connections.transport.name.cloudflare",
  relay: "connections.transport.name.relay",
};

export const transportName = (id: TransportId) => t(NAME_KEYS[id]);

/** A name inside a sentence ("Turn on your own relay"); it stays as is where it starts one or stands alone. */
export const midSentence = (name: string) => name.replace(/^Your /, "your ");

/** Daemon errors arrive without a final full stop; the fix hint after them needs one. */
const sentence = (text: string) => (/[.!?]$/.test(text) ? text : `${text}.`);

function tailscale(status: RemoteStatus): TransportView {
  const { installed, state, url, last_error } = status.tailscale;
  const enabled = status.configuration.tailscale.enabled;
  const configured = status.configuration.tailscale.public_url;
  if (!enabled) {
    if (!installed) return { id: "tailscale", phase: "off", url: null, hint: t("connections.transport.tailscale.install") };
    // The daemon reports a port held by another app before anything is turned on.
    return { id: "tailscale", phase: "off", url: null, hint: state === "failed" ? (last_error ?? null) : null };
  }
  if (state === "running") return { id: "tailscale", phase: "on", url: url ?? configured ?? null, hint: null };
  if (state === "stopped") {
    return { id: "tailscale", phase: "error", url: null, hint: t("connections.transport.tailscale.stopped") };
  }
  if (!installed) return { id: "tailscale", phase: "error", url: null, hint: t("connections.transport.tailscale.notFound") };
  return { id: "tailscale", phase: "error", url: null, hint: last_error ?? t("connections.transport.tailscale.failed") };
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
      hint: t("connections.transport.cloudflare.failed", { error: sentence(runtime.last_error ?? t("connections.transport.cloudflare.couldNotStart")) }),
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
      hint: t("connections.transport.relay.backoff", { error: sentence(runtime.last_error ?? t("connections.transport.relay.noAnswer")) }),
    };
  }
  return { id: "relay", phase: "starting", url: null, hint: null };
}

export function transportViews(status: RemoteStatus): TransportView[] {
  // Off: nothing runs and nothing may be offered as a way in, whatever settings are saved.
  if (!status.enabled) return (["tailscale", "cloudflare", "relay"] as const).map((id): TransportView => ({ id, phase: "off", url: null, hint: null }));
  return [tailscale(status), cloudflare(status), relay(status)];
}

/**
 * One suggestion so a first-time user is not left to compare three options. Tailscale wins when
 * it is already on this computer. Otherwise the relay: it opens nothing on the network, while a
 * tunnel needs a Cloudflare account and, for everyday use, a domain.
 */
export function recommendation(status: RemoteStatus): { id: TransportId; reason: string } {
  if (status.tailscale.installed) {
    return { id: "tailscale", reason: t("connections.recommend.tailscale") };
  }
  return { id: "relay", reason: t("connections.recommend.relay") };
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
  if (!status.enabled) return [];
  const on = transportViews(status).filter((view) => view.phase === "on" && view.url !== null);
  const rank = (id: TransportId) => {
    const index = status.modes.indexOf(id);
    return index === -1 ? status.modes.length : index;
  };
  const options = on
    .sort((a, b) => rank(a.id) - rank(b.id))
    .map((view): PairingOption => ({ via: view.id, label: transportName(view.id), url: view.url! }));
  if (options.length === 0 && status.public_url) options.push({ via: null, label: t("connections.pairing.savedAddress"), url: status.public_url });
  return options;
}

/** What stands between a phone and a QR code when no connection can carry one. */
export type ConnectionProblem =
  | { kind: "none" }
  | { kind: "starting"; id: TransportId; message: string }
  | { kind: "error"; id: TransportId; message: string };

const RETRYING: Record<TransportId, MessageKey> = {
  tailscale: "connections.problem.retrying.tailscale",
  cloudflare: "connections.problem.retrying.cloudflare",
  relay: "connections.problem.retrying.relay",
};

/** The tunnel was up before and went away, or a first start is still under way. */
function retrying(status: RemoteStatus, id: TransportId): boolean {
  if (id === "cloudflare") return status.cloudflare ? status.cloudflare.state === "restarting" || status.cloudflare.restarts > 0 : false;
  return false;
}

/**
 * Why there is no usable connection, from the one the person chose (`via`) or from every
 * connection that is turned on. A connection that is coming up wins over one that failed, because
 * it may fix itself while the person watches.
 */
export function connectionProblem(status: RemoteStatus, via: TransportId | null): ConnectionProblem {
  const views = transportViews(status).filter((view) => (via ? view.id === via : view.phase !== "off"));
  const starting = views.find((view) => view.phase === "starting");
  if (starting) {
    const message = retrying(status, starting.id)
      ? t(RETRYING[starting.id])
      : t("connections.problem.starting", { name: midSentence(transportName(starting.id)) });
    return { kind: "starting", id: starting.id, message };
  }
  const failed = views.find((view) => view.phase === "error");
  if (failed) return { kind: "error", id: failed.id, message: failed.hint ?? t("connections.problem.needsAttention", { name: transportName(failed.id) }) };
  const off = views.find((view) => view.phase === "off");
  // A chosen connection that was turned off meanwhile is as good as none turned on.
  return off && via ? { kind: "error", id: off.id, message: t("connections.problem.turnedOff", { name: transportName(off.id) }) } : { kind: "none" };
}

/**
 * The one connection a first-time person can turn on with a single click: Tailscale where it is
 * installed and its Serve port is free, a relay whose address is already saved, otherwise a
 * Cloudflare quick tunnel, which needs no account. A relay without an address cannot be one click.
 */
export function oneClickTransport(status: RemoteStatus): TransportId {
  if (status.tailscale.installed && status.tailscale.state !== "failed") return "tailscale";
  if (status.configuration.relay.url) return "relay";
  return "cloudflare";
}

/** Turns a connection on with the settings already saved for it, turning Remote Control on first when it is off. */
export async function enableTransport(client: Client, id: TransportId, status: RemoteStatus): Promise<RemoteStatus> {
  // The daemon refuses to start a connection while Remote Control is off.
  const current = status.enabled ? status : await client.enableRemote();
  // Turning Remote Control on already brought back a connection that was saved as on.
  if (!status.enabled && current.configuration[id].enabled) return current;
  const { cloudflare, relay } = current.configuration;
  if (id === "tailscale") return client.setTailscale(true);
  if (id === "relay") return client.setRelay({ enabled: true, url: relay.url ?? null, token_file: relay.token_file ?? null });
  const named = cloudflare.tunnel === "named" && !!cloudflare.hostname;
  return client.setCloudflare({ enabled: true, tunnel: named ? "named" : "quick", hostname: named ? cloudflare.hostname : null, token_file: cloudflare.token_file ?? null });
}

/**
 * Turns Remote Control on. The daemon brings back the connections saved for it; when none is
 * saved, the one-click connection is turned on as well.
 */
export async function turnOnRemote(client: Client, status: RemoteStatus): Promise<RemoteStatus> {
  const current = status.enabled ? status : await client.enableRemote();
  const { tailscale, cloudflare, relay } = current.configuration;
  return tailscale.enabled || cloudflare.enabled || relay.enabled ? current : enableTransport(client, oneClickTransport(current), current);
}
