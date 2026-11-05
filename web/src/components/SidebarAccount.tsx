import { ChevronsUpDown, MonitorSmartphone, Search, Wifi } from "lucide-react";
import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { ApiError } from "../api/client";
import type { MyDevice } from "../api/types";
import { useClient } from "../context";
import { manageLinkState } from "../pages/manageRoutes";
import { useShell } from "../shell";
import { keyLabel, searchKeys } from "../shortcuts";
import { ICON_STROKE, IconButton, MenuItem, PopoverMenu } from "../ui";
import { Avatar } from "./Avatar";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** Initials of a pairing label: "Work laptop" → "WL", "phone" → "P". */
function deviceInitials(name: string): string {
  const words = name.match(/[\p{L}\p{N}]+/gu) ?? [];
  return words
    .slice(0, 2)
    .map((word) => word.charAt(0).toUpperCase())
    .join("");
}

/**
 * The rail's bottom utility row: this device's identity (the only account openside has) with a
 * menu into Devices and Remote access, and the global search palette. Stays outside the
 * scrolling list so it is reachable however long the roster grows.
 */
export function SidebarAccount({ onClose }: { onClose?: () => void }) {
  const client = useClient();
  const shell = useShell();
  const location = useLocation();
  const navigate = useNavigate();
  const [device, setDevice] = useState<MyDevice | null>(null);

  // Until the identity is known the row keeps only Search rather than inventing a name. A 4xx
  // (404 from a daemon without the route, a refused token) is final; a network failure or 5xx is
  // asked again the next time the page is focused or becomes visible.
  useEffect(() => {
    let current = true;
    let settled = false;
    let inFlight = false;
    const load = () => {
      if (settled || inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      client.myDevice().then(
        (me) => {
          settled = true;
          if (current) setDevice(me);
        },
        (cause: unknown) => {
          // The client reports an unreachable daemon as status 0.
          if (cause instanceof ApiError && cause.status >= 400 && cause.status < 500) settled = true;
        },
      ).finally(() => {
        inFlight = false;
      });
    };
    load();
    window.addEventListener("focus", load);
    document.addEventListener("visibilitychange", load);
    return () => {
      current = false;
      window.removeEventListener("focus", load);
      document.removeEventListener("visibilitychange", load);
    };
  }, [client]);

  const open = (path: "/devices" | "/remote") => () => navigate(path, { state: manageLinkState(location) });
  const search = () => {
    onClose?.();
    shell.openSearch();
  };
  const initials = device ? deviceInitials(device.name) : "";

  return (
    <div className="sidebar-utility">
      {device && (
        <PopoverMenu
          label="Account"
          title={device.name}
          side="top"
          align="start"
          trigger={
            <button type="button" className="sidebar-row sidebar-account" aria-label={`Account: ${device.name}`} title={device.name}>
              <Avatar initials={initials || "?"} color="gray" size={24} />
              <span className="sidebar-account-name">{device.name}</span>
              <ChevronsUpDown {...icon} />
            </button>
          }
        >
          <MenuItem icon={<MonitorSmartphone {...icon} />} onClick={open("/devices")}>
            Devices
          </MenuItem>
          <MenuItem icon={<Wifi {...icon} />} onClick={open("/remote")}>
            Remote access
          </MenuItem>
        </PopoverMenu>
      )}
      <IconButton label="Search" title={`Search (${keyLabel(searchKeys())})`} className="sidebar-search" onClick={search}>
        <Search size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </IconButton>
    </div>
  );
}
