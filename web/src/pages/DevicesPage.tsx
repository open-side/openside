import { MonitorSmartphone } from "lucide-react";
import { useCallback, useState } from "react";
import { thisDevice } from "../auth";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Segmented, useToast } from "../ui";
import { EmptyState, ResourceCard, ResourceView, SectionList, useConfirm } from "./blocks";
import { formatDate } from "./format";

/** Paired devices (revocable) and how to pair another one. */
export function DevicesPage() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listDevices(), [client]);
  const devices = useResource(load);
  const [confirm, ask] = useConfirm();
  const self = thisDevice();
  const [purpose, setPurpose] = useState<"person_device" | "agent_client">("person_device");
  const agent = purpose === "agent_client";
  const pairCommand = agent ? 'openside pair --agent --name "My agent"' : 'openside pair --name "My phone"';

  async function revoke(id: string) {
    try {
      await client.revokeDevice(id);
      toast.notify("Device revoked.");
      await devices.reload();
    } catch (cause) {
      devices.setError(errorMessage(cause));
    }
  }

  return (
    <div className="manage-body" data-section="devices">
      <SectionList title="Paired devices" label="Paired devices">
        <ResourceView resource={devices} what="paired devices">
          {(data) =>
            data.devices.length === 0 ? (
              <EmptyState icon={MonitorSmartphone} title="No paired devices" detail="Devices you pair with openside pair appear here." />
            ) : (
              <ul className="resource-list" aria-label="Devices">
                {data.devices.map((device) => {
                  const name = device.name || "Unnamed device";
                  return (
                    <li key={device.device_id} data-device={device.device_id}>
                      <ResourceCard
                        icon={MonitorSmartphone}
                        title={name}
                        detail={`${device.kind === "agent_client" ? "Agent client" : "Person’s device"} · ${device.created_at ? `Paired ${formatDate(device.created_at)} · ` : ""}${device.device_id}`}
                        badge={device.device_id === self ? "This device" : undefined}
                        trailing={
                          <Button
                            size="compact"
                            variant="ghost"
                            className="row-danger"
                            onClick={() =>
                              ask({
                                title: `Revoke ${name}?`,
                                description:
                                  device.device_id === self
                                    ? "This is the device you are using; it will be signed out."
                                    : "Its token stops working immediately. Pair it again to reconnect.",
                                confirmLabel: "Revoke",
                                onConfirm: () => void revoke(device.device_id),
                              })
                            }
                          >
                            Revoke
                          </Button>
                        }
                      >
                        <div className="resource-card-meta" aria-label="Scopes">
                          {device.scopes.map((scope) => (
                            <span key={scope} className="tag">
                              {scope}
                            </span>
                          ))}
                        </div>
                      </ResourceCard>
                    </li>
                  );
                })}
              </ul>
            )
          }
        </ResourceView>
      </SectionList>
      <SectionList title="Pair another device" label="Pair a device">
        <Segmented
          label="Pairing purpose"
          options={[
            { value: "person_device", label: "Pair a phone/browser" },
            { value: "agent_client", label: "Connect an agent / CLI" },
          ]}
          value={purpose}
          onChange={setPurpose}
        />
        <p className="section-list-description">
          {agent
            ? "An agent must wait for a person’s approval on another paired device. The link fixes this role; the client cannot change it."
            : "This device conveys your approvals. Keep its token for yourself; do not give it to an agent."}
        </p>
        <ol className="pair-steps">
          <li>
            On the computer running openside, run <code>{pairCommand}</code>.
            {agent
              ? " Its default scopes are read, steer, browse and approve."
              : <> Its default scopes are read, approve and human_step; add <code>--scope steer</code> to start conversations and <code>--scope admin</code> to manage settings.</>}
          </li>
          <li>
            {agent
              ? <>On the agent’s computer, run <code>openside connect</code> and paste the printed link at the hidden prompt. Start MCP with <code>openside --remote &lt;name&gt; mcp</code>.</>
              : "Scan the QR code with the new device, or open the printed link in its browser."}
          </li>
          <li>The link signs the device in once; the code expires after 3 minutes. Its role is fixed when the link is issued.</li>
        </ol>
      </SectionList>
      {confirm}
    </div>
  );
}
