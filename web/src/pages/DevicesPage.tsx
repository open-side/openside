import { MonitorSmartphone } from "lucide-react";
import { useCallback } from "react";
import { thisDevice } from "../auth";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, useToast } from "../ui";
import { CardActions, EmptyState, ErrorNote, Loading, ResourceCard, SectionList, useConfirm } from "./blocks";

/** Paired devices (revocable) and how to pair another one. */
export function DevicesPage() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listDevices(), [client]);
  const devices = useResource(load);
  const [confirm, ask] = useConfirm();
  const self = thisDevice();

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
        {devices.error ? <ErrorNote>{devices.error}</ErrorNote> : null}
        {!devices.data ? (
          devices.error ? null : <Loading />
        ) : devices.data.devices.length === 0 ? (
          <EmptyState icon={MonitorSmartphone} title="No paired devices" />
        ) : (
          <ul className="resource-list" aria-label="Devices">
            {devices.data.devices.map((device) => (
              <li key={device.device_id} data-device={device.device_id}>
                <ResourceCard
                  title={device.name || device.device_id}
                  detail={`${device.scopes.join(", ")}${device.created_at ? ` · paired ${new Date(device.created_at).toLocaleString()}` : ""}`}
                  badge={device.device_id === self ? "this device" : undefined}
                >
                  <CardActions>
                    <Button
                      size="compact"
                      variant="danger"
                      onClick={() =>
                        ask({
                          title: `Revoke ${device.name || device.device_id}?`,
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
                  </CardActions>
                </ResourceCard>
              </li>
            ))}
          </ul>
        )}
      </SectionList>
      <SectionList title="Pair another device" label="Pair a device">
        <ol className="pair-steps">
          <li>
            On the computer running openside, run <code>openside pair --name "My phone"</code>. The default scopes are read,
            approve and human_step; add <code>--scope steer</code> to start conversations and <code>--scope admin</code> for
            skills, routines, memory, vault, devices and settings.
          </li>
          <li>Scan the QR code with the new device, or open the printed link in its browser.</li>
          <li>The link signs the device in once; the code expires after 3 minutes.</li>
        </ol>
      </SectionList>
      {confirm}
    </div>
  );
}
