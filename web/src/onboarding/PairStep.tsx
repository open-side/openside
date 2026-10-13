import { useCallback, useEffect } from "react";
import type { Device } from "../api/types";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { ErrorNote, Loading } from "../pages/blocks";
import { ScopeNotice } from "../pages/ScopeNotice";
import { ConnectPhone } from "../remote/ConnectPhone";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button } from "../ui";
import type { StepProps } from "./summary";

const STATUS_POLL_MS = 4_000;

export function PairStep({ update }: StepProps) {
  const client = useClient();
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
  return <ConnectPhone status={status} onStatusStale={() => void reload()} onPaired={paired} />;
}
