import { Check, Copy, Smartphone } from "lucide-react";
import { useCallback, useEffect, useRef } from "react";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { ResourceView } from "../pages/blocks";
import { Button, copyText, ICON_STROKE, Spinner, useToast } from "../ui";
import type { StepProps } from "./summary";

/** Pairing codes can only be issued on the computer itself, so this step waits for the result. */
const POLL_MS = 3_000;
const COMMAND = 'openside pair --name "My phone"';

export function PairStep({ update }: StepProps) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listDevices(), [client]);
  const devices = useResource(load);
  const { data, reload } = devices;
  // Devices that were there when the step opened; anything else is the phone being paired.
  const known = useRef<Set<string> | null>(null);

  useEffect(() => {
    const timer = window.setInterval(() => void reload(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [reload]);

  const added = data && known.current ? data.devices.filter((device) => !known.current!.has(device.device_id)) : [];
  if (data && known.current === null) known.current = new Set(data.devices.map((device) => device.device_id));
  const phone = added[0]?.name || (added[0] ? "New device" : null);

  useEffect(() => {
    if (phone) update({ phone });
  }, [phone, update]);

  async function copy() {
    toast.notify((await copyText(COMMAND)) ? "Command copied" : "Could not copy; select the command instead", "success");
  }

  return (
    <>
      <ol className="onb-steps">
        <li>
          <span>On this computer, run</span>
          <span className="onb-command">
            <code>{COMMAND}</code>
            <Button size="compact" variant="ghost" aria-label="Copy the pairing command" onClick={() => void copy()}>
              <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Copy
            </Button>
          </span>
        </li>
        <li>Scan the QR code it prints with your phone’s camera and open the link.</li>
        <li>Keep this page open. It notices when the phone connects.</li>
      </ol>
      <p className="onb-note">
        This link pairs a person’s device for approvals. For an external CLI/MCP agent, issue a separate link with <code>openside pair --agent</code>.
      </p>
      <ResourceView resource={devices} what="paired devices">
        {() =>
          phone ? (
            <p className="onb-result onb-result-title" role="status">
              <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> {phone} is paired
            </p>
          ) : (
            <p className="onb-waiting" role="status">
              <Spinner size={16} />
              <span>Waiting for a new device…</span>
            </p>
          )
        }
      </ResourceView>
      <p className="onb-note">
        <Smartphone size={14} strokeWidth={ICON_STROKE} aria-hidden="true" /> The phone needs to reach this computer, on the same
        network or over a private network such as Tailscale. Run <code>openside remote setup</code> to open another route.
      </p>
    </>
  );
}
