import { ChevronDown, Cloud, Copy, Info, Network, Server } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useCallback, useEffect, useId, useState, type ReactNode } from "react";
import type { RemoteStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { formatList, t, type MessageKey } from "../i18n";
import { RestartNotice } from "../pages/RestartNotice";
import { ErrorNote, ResourceCard, ResourceView, useConfirm, type BadgeTone } from "../pages/blocks";
import { Button, copyText, Dialog, ICON_STROKE, Segmented, SettingsCard, SettingsRow, SettingsSection, Spinner, Switch, useToast } from "../ui";
import { ConnectPhone } from "./ConnectPhone";
import { LocalOnlyNotice } from "./LocalOnlyNotice";
import { midSentence, recommendation, transportName, transportViews, turnOnRemote, type TransportId, type TransportPhase, type TransportView } from "./transports";
import "./remote.css";

const POLL_MS = 4_000;

const ICONS: Record<TransportId, LucideIcon> = { tailscale: Network, cloudflare: Cloud, relay: Server };

const SUMMARY: Record<TransportId, MessageKey> = {
  tailscale: "connections.remote.summary.tailscale",
  cloudflare: "connections.remote.summary.cloudflare",
  relay: "connections.remote.summary.relay",
};

const PHASE_LABEL: Record<TransportPhase, MessageKey> = {
  off: "common.off",
  starting: "connections.remote.phase.starting",
  on: "common.on",
  error: "connections.remote.phase.error",
};
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
      setError(t("connections.remote.hostnameRequired"));
      return;
    }
    if (enabled && id === "relay" && !relayUrl.trim()) {
      setError(t("connections.remote.relayRequired"));
      return;
    }
    setBusy(true);
    try {
      // The daemon starts no connection while Remote Control is off, so turning one on turns it on first.
      if (enabled && !status.enabled) await client.enableRemote();
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
    toast.notify((await copyText(url)) ? t("connections.remote.addressCopied") : t("connections.remote.copyFailed"), "success");
  }

  const on = view.phase !== "off";
  const name = transportName(id);
  return (
    <ResourceCard
      icon={ICONS[id]}
      title={
        <>
          <span>{name}</span>
          {recommended && <span className="chip">{t("connections.remote.recommended")}</span>}
        </>
      }
      badge={t(PHASE_LABEL[view.phase])}
      badgeTone={PHASE_TONE[view.phase]}
      detail={t(SUMMARY[id])}
      data-transport={id}
      data-phase={view.phase}
    >
      {recommended && view.phase === "off" && <p className="remote-reco">{reason}</p>}
      {view.phase === "starting" && (
        <p className="remote-waiting" role="status">
          <Spinner size={14} />
          <span>{t("connections.remote.startingWait")}</span>
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
          <Button variant="ghost" aria-label={t("connections.remote.copyAddress", { name: midSentence(name) })} onClick={() => void copy(view.url!)}>
            <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("common.copy")}
          </Button>
        </div>
      )}
      {id === "cloudflare" && canConfigure && (
        <div className="remote-fields">
          <Segmented
            label={t("connections.remote.tunnelType")}
            options={[
              { value: "quick", label: t("connections.remote.tunnel.quick") },
              { value: "named", label: t("connections.remote.tunnel.named") },
            ]}
            value={tunnel}
            onChange={setTunnel}
          />
          {tunnel === "quick" ? (
            <p className="remote-note">{t("connections.remote.quickNote")}</p>
          ) : (
            <>
              <label className="remote-field">
                <span>{t("connections.remote.hostname")}</span>
                <input value={hostname} placeholder="desk.example.com" spellCheck={false} onChange={(event) => setHostname(event.target.value)} />
              </label>
              <label className="remote-field">
                <span>{t("connections.remote.tokenFile")}</span>
                <input
                  value={cloudflareToken}
                  placeholder="/home/you/.config/openside/cloudflare-token"
                  spellCheck={false}
                  onChange={(event) => setCloudflareToken(event.target.value)}
                />
                <small>{t("connections.remote.cloudflareTokenHint")}</small>
              </label>
            </>
          )}
        </div>
      )}
      {id === "relay" && canConfigure && (
        <div className="remote-fields">
          <label className="remote-field">
            <span>{t("connections.remote.relayAddress")}</span>
            <input value={relayUrl} placeholder="https://relay.example.com" spellCheck={false} inputMode="url" onChange={(event) => setRelayUrl(event.target.value)} />
          </label>
          <label className="remote-field">
            <span>{t("connections.remote.tokenFile")}</span>
            <input
              value={relayToken}
              placeholder="/home/you/.config/openside/relay-token"
              spellCheck={false}
              onChange={(event) => setRelayToken(event.target.value)}
            />
            <small>{t("connections.remote.relayTokenHint")}</small>
          </label>
        </div>
      )}
      {error && <ErrorNote>{error}</ErrorNote>}
      {canConfigure ? (
        <div className="remote-actions">
          {on ? (
            <>
              {dirty && (
                <Button variant="primary" disabled={busy} onClick={() => void apply(true)}>
                  {t("connections.remote.saveChanges")}
                </Button>
              )}
              {view.phase === "error" && !dirty && (
                <Button variant="primary" disabled={busy} onClick={() => void apply(true)}>
                  {t("common.tryAgain")}
                </Button>
              )}
              <Button variant="secondary" disabled={busy} onClick={() => void apply(false)}>
                {t("connections.transport.turnOff", { name: midSentence(name) })}
              </Button>
            </>
          ) : (
            <Button variant={recommended ? "primary" : "secondary"} disabled={busy} onClick={() => void apply(true)}>
              {busy && <Spinner size={14} />}
              {t("connections.transport.turnOn", { name: midSentence(name) })}
            </Button>
          )}
        </div>
      ) : null}
    </ResourceCard>
  );
}

/** A small ⓘ link under a settings row's text that opens its help (Remote Control, MCP server). */
export function HelpLink({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" className="help-link" onClick={onClick}>
      <Info strokeWidth={ICON_STROKE} aria-hidden="true" />
      {children}
    </button>
  );
}

function RemoteControl({ status, onStatus, onStale }: { status: RemoteStatus; onStatus: (next: RemoteStatus) => void; onStale: () => void }) {
  const client = useClient();
  const advancedId = useId();
  const views = transportViews(status);
  const pick = recommendation(status);
  const anyOn = views.some((view) => view.phase !== "off");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [phone, setPhone] = useState(false);
  const [help, setHelp] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [confirm, ask] = useConfirm();

  async function change(turnOn: boolean) {
    setBusy(true);
    setError(null);
    try {
      if (turnOn) {
        onStatus(await turnOnRemote(client, status));
      } else {
        // The daemon refuses remote traffic at once and keeps every connection's settings; it
        // names any connection it could not stop, which the person may have to stop by hand.
        const { status: next, still_on } = await client.disableRemote();
        onStatus(next);
        if (still_on.length > 0) setError(t("settings.developers.remote.stillOn", { names: formatList(still_on.map(transportName)) }));
      }
    } catch (cause) {
      setError(errorMessage(cause));
      onStale();
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      {!status.configure_allowed && <LocalOnlyNotice what={t("connections.remote.localOnlyWhat")} onChanged={onStale} />}
      {status.restart_required && <RestartNotice onRestarted={onStale}>{t("connections.remote.restartSaved")}</RestartNotice>}
      <SettingsCard>
        <SettingsRow
          title={t("settings.developers.remote.allow.title")}
          description={
            <>
              <span className="remote-control-line">{t("settings.developers.remote.allow.description")}</span>
              <HelpLink onClick={() => setHelp(true)}>{t("settings.developers.remote.how")}</HelpLink>
            </>
          }
          control={(ids) => (
            <Switch
              aria-labelledby={ids.labelId}
              checked={status.enabled}
              disabled={busy || !status.configure_allowed}
              onChange={(on) =>
                on
                  ? void change(true)
                  : ask({
                      title: t("settings.developers.remote.offTitle"),
                      description: t("settings.developers.remote.offDescription"),
                      confirmLabel: t("settings.developers.remote.offConfirm"),
                      onConfirm: () => void change(false),
                    })
              }
            />
          )}
        />
        <SettingsRow title={t("connections.remote.connectPhone")} description={t("settings.developers.remote.phone.description")} onClick={() => setPhone(true)} />
        <SettingsRow title={t("connections.remote.pairedDevices")} description={t("connections.remote.pairedDevicesDescription")} to="/settings/account" />
        <button type="button" className="ui-row ui-row--single ui-row--action remote-advanced-toggle" aria-expanded={advanced} aria-controls={advancedId} onClick={() => setAdvanced(!advanced)}>
          <span className="ui-row-copy">
            <span className="ui-row-title">{t("settings.developers.remote.advanced")}</span>
          </span>
          <ChevronDown className="ui-row-glyph remote-advanced-chevron" strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
        <div className="remote-advanced" id={advancedId} role="region" aria-label={t("settings.developers.remote.advanced")} hidden={!advanced}>
          {advanced && (
            <>
              <p className="remote-note">{t("settings.developers.remote.advanced.description")}</p>
              <ul className="resource-list" aria-label={t("connections.remote.listLabel")}>
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
            </>
          )}
        </div>
      </SettingsCard>
      {error && <ErrorNote>{error}</ErrorNote>}
      <Dialog open={phone} onClose={() => setPhone(false)} title={t("connections.remote.connectPhone")}>
        <ConnectPhone status={status} onStatusStale={onStale} />
      </Dialog>
      <Dialog open={help} onClose={() => setHelp(false)} title={t("settings.developers.remote.howTitle")}>
        <div className="remote-help">
          <p>{t("settings.developers.remote.howBody.computer")}</p>
          <p>{t("settings.developers.remote.howBody.pairing")}</p>
          <p>{t("settings.developers.remote.howBody.stays")}</p>
        </div>
      </Dialog>
      {confirm}
    </>
  );
}

/**
 * Settings › Developers, Remote Control: one switch for the connection a first-time person needs,
 * the phone and paired-device flows, and every transport under Advanced.
 */
export function RemoteControlSection() {
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
    <SettingsSection label={t("settings.developers.remote.label")}>
      <ResourceView resource={status ? { ...resource, data: status } : resource} what={t("connections.remote.what")}>
        {(data) => <RemoteControl status={data} onStatus={(next) => setSaved({ base: resource.data, value: next })} onStale={() => void reload()} />}
      </ResourceView>
    </SettingsSection>
  );
}
