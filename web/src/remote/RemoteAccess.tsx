import { Cloud, Copy, Network, Server } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Link, useLocation } from "react-router";
import type { RemoteStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { manageLinkState } from "../pages/manageRoutes";
import { RestartNotice } from "../pages/RestartNotice";
import { ErrorNote, ResourceCard, ResourceView, SectionList, type BadgeTone } from "../pages/blocks";
import { Button, copyText, ICON_STROKE, Segmented, Spinner, useToast } from "../ui";
import { ConnectPhone } from "./ConnectPhone";
import { LocalOnlyNotice } from "./LocalOnlyNotice";
import { recommendation, TRANSPORT_NAMES, transportViews, type TransportId, type TransportPhase, type TransportView } from "./transports";
import "./remote.css";

const POLL_MS = 4_000;

const ICONS: Record<TransportId, LucideIcon> = { tailscale: Network, cloudflare: Cloud, relay: Server };

const SUMMARY: Record<TransportId, string> = {
  tailscale: "Private. Your phone reaches this computer through your Tailscale account, and nothing is opened to the internet.",
  cloudflare: "A public web address that works from anywhere. Traffic passes through Cloudflare.",
  relay:
    "This computer connects out to a relay server, so no ports are opened. The relay cannot read your traffic, but it serves the app itself: run it yourself or trust whoever does.",
};

const PHASE_LABEL: Record<TransportPhase, string> = { off: "Off", starting: "Starting…", on: "On", error: "Needs attention" };
const PHASE_TONE: Record<TransportPhase, BadgeTone> = { off: "neutral", starting: "attention", on: "success", error: "danger" };

function TransportCard({
  view,
  status,
  recommended,
  reason,
  canConfigure,
  onStatus,
}: {
  view: TransportView;
  status: RemoteStatus;
  recommended: boolean;
  reason: string;
  canConfigure: boolean;
  onStatus: (next: RemoteStatus) => void;
}) {
  const client = useClient();
  const toast = useToast();
  const { cloudflare, relay } = status.configuration;
  const [tunnel, setTunnel] = useState<"quick" | "named">(cloudflare.tunnel === "named" ? "named" : "quick");
  const [hostname, setHostname] = useState(cloudflare.hostname ?? "");
  const [cloudflareToken, setCloudflareToken] = useState(cloudflare.token_file ?? "");
  const [relayUrl, setRelayUrl] = useState(relay.url ?? "");
  const [relayToken, setRelayToken] = useState(relay.token_file ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const id = view.id;

  const dirty =
    id === "cloudflare"
      ? tunnel !== cloudflare.tunnel || hostname.trim() !== (cloudflare.hostname ?? "") || cloudflareToken.trim() !== (cloudflare.token_file ?? "")
      : id === "relay"
        ? relayUrl.trim() !== (relay.url ?? "") || relayToken.trim() !== (relay.token_file ?? "")
        : false;

  async function apply(enabled: boolean) {
    setError(null);
    if (enabled && id === "cloudflare" && tunnel === "named" && !hostname.trim()) {
      setError("Enter the hostname your tunnel serves, such as desk.example.com.");
      return;
    }
    if (enabled && id === "relay" && !relayUrl.trim()) {
      setError("Enter the address of your relay, such as https://relay.example.com.");
      return;
    }
    setBusy(true);
    try {
      const next =
        id === "tailscale"
          ? await client.setTailscale(enabled)
          : id === "cloudflare"
            ? await client.setCloudflare({
                enabled,
                tunnel,
                hostname: tunnel === "named" && hostname.trim() ? hostname.trim() : null,
                token_file: cloudflareToken.trim() || null,
              })
            : await client.setRelay({ enabled, url: relayUrl.trim() || null, token_file: relayToken.trim() || null });
      onStatus(next);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copy(url: string) {
    toast.notify((await copyText(url)) ? "Address copied" : "Could not copy; select the address instead", "success");
  }

  const on = view.phase !== "off";
  const name = TRANSPORT_NAMES[id];
  return (
    <ResourceCard
      icon={ICONS[id]}
      title={
        <>
          <span>{name}</span>
          {recommended && <span className="chip">Recommended</span>}
        </>
      }
      badge={PHASE_LABEL[view.phase]}
      badgeTone={PHASE_TONE[view.phase]}
      detail={SUMMARY[id]}
      data-transport={id}
      data-phase={view.phase}
    >
      {recommended && view.phase === "off" && <p className="remote-reco">{reason}</p>}
      {view.phase === "starting" && (
        <p className="remote-waiting" role="status">
          <Spinner size={14} />
          <span>Starting… this can take a few seconds.</span>
        </p>
      )}
      {view.hint && (
        <p className={view.phase === "error" ? "remote-hint remote-hint--error" : "remote-hint"} role={view.phase === "error" ? "alert" : undefined}>
          {view.hint}
        </p>
      )}
      {view.url && (
        <div className="remote-url">
          <code>{view.url}</code>
          <Button size="compact" variant="ghost" aria-label={`Copy the ${name} address`} onClick={() => void copy(view.url!)}>
            <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            Copy
          </Button>
        </div>
      )}
      {id === "cloudflare" && canConfigure && (
        <div className="remote-fields">
          <Segmented
            label="Cloudflare tunnel type"
            options={[
              { value: "quick", label: "Quick" },
              { value: "named", label: "Your domain" },
            ]}
            value={tunnel}
            onChange={setTunnel}
          />
          {tunnel === "quick" ? (
            <p className="remote-note">No Cloudflare account needed. The address changes every time it starts, so make a new QR code after a restart.</p>
          ) : (
            <>
              <label className="remote-field">
                <span>Hostname</span>
                <input value={hostname} placeholder="desk.example.com" spellCheck={false} onChange={(event) => setHostname(event.target.value)} />
              </label>
              <label className="remote-field">
                <span>Token file</span>
                <input
                  value={cloudflareToken}
                  placeholder="/home/you/.config/openside/cloudflare-token"
                  spellCheck={false}
                  onChange={(event) => setCloudflareToken(event.target.value)}
                />
                <small>A file on this computer that holds your tunnel token. The token itself never goes through this screen.</small>
              </label>
            </>
          )}
        </div>
      )}
      {id === "relay" && canConfigure && (
        <div className="remote-fields">
          <label className="remote-field">
            <span>Relay address</span>
            <input value={relayUrl} placeholder="https://relay.example.com" spellCheck={false} inputMode="url" onChange={(event) => setRelayUrl(event.target.value)} />
          </label>
          <label className="remote-field">
            <span>Token file</span>
            <input
              value={relayToken}
              placeholder="/home/you/.config/openside/relay-token"
              spellCheck={false}
              onChange={(event) => setRelayToken(event.target.value)}
            />
            <small>A file on this computer that holds the token your relay gave you. Leave it empty to use the OPENSIDE_RELAY_TOKEN variable.</small>
          </label>
        </div>
      )}
      {error && <ErrorNote>{error}</ErrorNote>}
      {canConfigure ? (
        <div className="remote-actions">
          {on ? (
            <>
              {dirty && (
                <Button variant="primary" size="compact" disabled={busy} onClick={() => void apply(true)}>
                  Save changes
                </Button>
              )}
              {view.phase === "error" && !dirty && (
                <Button variant="primary" size="compact" disabled={busy} onClick={() => void apply(true)}>
                  Try again
                </Button>
              )}
              <Button variant="secondary" size="compact" disabled={busy} onClick={() => void apply(false)}>
                Turn off {name}
              </Button>
            </>
          ) : (
            <Button variant={recommended ? "primary" : "secondary"} size="compact" disabled={busy} onClick={() => void apply(true)}>
              {busy && <Spinner size={14} />}
              Turn on {name}
            </Button>
          )}
        </div>
      ) : null}
    </ResourceCard>
  );
}

function RemoteBody({ status, onStatus, onStale }: { status: RemoteStatus; onStatus: (next: RemoteStatus) => void; onStale: () => void }) {
  const location = useLocation();
  const views = transportViews(status);
  const pick = recommendation(status);
  const anyOn = views.some((view) => view.phase !== "off");
  return (
    <>
      <SectionList title="Connect your phone" label="Connect your phone">
        <ConnectPhone status={status} onStatusStale={onStale} />
      </SectionList>
      <SectionList
        title="Ways to reach this computer"
        description="Turn on one way in. Your phone then connects to openside from anywhere, using the QR code above."
      >
        {!status.configure_allowed && <LocalOnlyNotice what="change these connections" onChanged={onStale} />}
        {status.restart_required && <RestartNotice onRestarted={onStale}>Saved. Restart openside to finish turning this on.</RestartNotice>}
        <ul className="resource-list" aria-label="Connections">
          {views.map((view) => (
            <li key={view.id}>
              <TransportCard
                view={view}
                status={status}
                recommended={!anyOn && view.id === pick.id}
                reason={pick.reason}
                canConfigure={status.configure_allowed}
                onStatus={onStatus}
              />
            </li>
          ))}
        </ul>
      </SectionList>
      <SectionList title="Paired devices" label="Paired devices link">
        <p className="section-list-description">Phones and agents that are connected now, with the option to revoke any of them.</p>
        <div className="remote-actions">
          <Link className="ui-button ui-button--secondary ui-button--compact" to="/devices" replace state={manageLinkState(location)}>
            See paired devices
          </Link>
        </div>
      </SectionList>
    </>
  );
}

/** Settings → Remote access: turn connections on, then connect a phone with a QR code. */
export function RemoteAccess() {
  const client = useClient();
  const load = useCallback(() => client.remoteStatus(), [client]);
  const resource = useResource(load);
  const { reload } = resource;
  // The answer to a Turn on/off call is fresher than the last poll until the next poll lands.
  const [saved, setSaved] = useState<{ base: RemoteStatus | null; value: RemoteStatus } | null>(null);
  const status = saved && saved.base === resource.data ? saved.value : resource.data;

  useEffect(() => {
    const timer = window.setInterval(() => void reload(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [reload]);

  return (
    <div className="manage-body" data-section="remote">
      <ResourceView resource={status ? { ...resource, data: status } : resource} what="remote access">
        {(data) => <RemoteBody status={data} onStatus={(next) => setSaved({ base: resource.data, value: next })} onStale={() => void reload()} />}
      </ResourceView>
    </div>
  );
}
