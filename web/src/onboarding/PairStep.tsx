import { Smartphone } from "lucide-react";
import { useCallback, useEffect } from "react";
import { Link, useLocation } from "react-router";
import type { Device } from "../api/types";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { ErrorNote, Loading } from "../pages/blocks";
import { ScopeNotice } from "../pages/ScopeNotice";
import { manageLinkState } from "../pages/manageRoutes";
import { ConnectPhone } from "../remote/ConnectPhone";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { pairingOptions } from "../remote/transports";
import { Button, ICON_STROKE } from "../ui";
import type { StepProps } from "./summary";

const STATUS_POLL_MS = 4_000;

export function PairStep({ update }: StepProps) {
  const client = useClient();
  const location = useLocation();
  const load = useCallback(() => client.remoteStatus(), [client]);
  const remote = useResource(load);
  const { data: status, reload } = remote;
  const paired = useCallback((device: Device) => update({ phone: device.name || "New device" }), [update]);

  // Remote access can be turned on in a dialog over this guide; pick the change up without a reload.
  useEffect(() => {
    const timer = window.setInterval(() => void reload(), STATUS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [reload]);

  if (!status) {
    if (remote.forbidden) return <ScopeNotice what="phone pairing" onGranted={() => void reload()} />;
    return remote.error ? (
      <div className="onb-problem">
        <ErrorNote>{remote.error}</ErrorNote>
        <Button size="compact" onClick={() => void reload()}>
          Try again
        </Button>
      </div>
    ) : (
      <Loading />
    );
  }
  if (!status.pairing_allowed) return <LocalOnlyNotice what="connect a phone" onChanged={() => void reload()} />;
  if (pairingOptions(status).length === 0) {
    return (
      <>
        <p className="onb-note">
          <Smartphone size={14} strokeWidth={ICON_STROKE} aria-hidden="true" /> A phone needs a way to reach this computer first: Tailscale, a
          Cloudflare tunnel or your own relay.
        </p>
        <div className="onb-actions">
          <Link className="ui-button ui-button--primary" to="/remote" state={manageLinkState(location)}>
            Set up remote access
          </Link>
        </div>
      </>
    );
  }
  return <ConnectPhone status={status} onStatusStale={() => void reload()} onPaired={paired} />;
}
