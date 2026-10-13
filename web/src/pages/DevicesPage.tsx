import { MonitorSmartphone } from "lucide-react";
import { useCallback, useState } from "react";
import { Link, useLocation } from "react-router";
import { thisDevice } from "../auth";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Segmented, useToast } from "../ui";
import "../remote/remote.css";
import { EmptyState, ResourceCard, ResourceView, SectionList, useConfirm } from "./blocks";
import { AgentLink } from "./AgentLink";
import { CliHint } from "./CliHint";
import { formatDate } from "./format";

/** Paired devices (revocable) and how to pair another one. */
export function DevicesPage() {
  const client = useClient();
  const toast = useToast();
  const location = useLocation();
  const load = useCallback(() => client.listDevices(), [client]);
  const devices = useResource(load);
  const [confirm, ask] = useConfirm();
  const self = thisDevice();
  const [purpose, setPurpose] = useState<"person_device" | "agent_client">("person_device");
  const agent = purpose === "agent_client";

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
              <EmptyState icon={MonitorSmartphone} title="No paired devices" detail="Phones and agents you connect appear here." />
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
        {agent ? null : (
          <p className="section-list-description">This device conveys your approvals. Keep its token for yourself; do not give it to an agent.</p>
        )}
        {agent ? (
          <AgentLink />
        ) : (
          <>
            <div className="remote-actions">
              <Link className="ui-button ui-button--primary" to="/remote" replace state={location.state}>
                Connect a phone with a QR code
              </Link>
            </div>
            <CliHint command='openside pair --name "My phone"' />
          </>
        )}
      </SectionList>
      {confirm}
    </div>
  );
}
