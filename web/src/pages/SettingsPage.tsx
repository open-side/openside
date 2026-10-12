import { ChevronRight, Compass, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";
import type { Health } from "../api/types";
import { AppearanceSettings } from "../components/AppearanceSettings";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { disableNotifications, enableNotifications, pushSupported, storedSubscription, subscriptionActive } from "../push";
import { Button, ICON_STROKE } from "../ui";
import { Badge, ErrorNote, StatusRow } from "./blocks";

/** aside-relay re-checks the connection every five seconds while Settings is open. */
const HEALTH_INTERVAL_MS = 5_000;

type PushStatus = "checking" | "enabled" | "off" | "denied" | "unavailable" | "install";

const PUSH_LABEL: Record<PushStatus, string> = {
  checking: "Checking…",
  enabled: "Enabled",
  off: "Off",
  denied: "Blocked",
  unavailable: "Unavailable",
  install: "Home Screen app only",
};

/** iOS delivers Web Push only to Home Screen apps; a Safari tab has no PushManager at all. */
function iosBrowserTab(): boolean {
  const ios = /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  const standalone =
    (typeof window.matchMedia === "function" && window.matchMedia("(display-mode: standalone)").matches) ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true;
  return ios && !standalone;
}

function initialPushStatus(): PushStatus {
  if (iosBrowserTab()) return "install";
  if (!pushSupported()) return "unavailable";
  if (Notification.permission === "denied") return "denied";
  return storedSubscription() ? "checking" : "off";
}

function Notifications() {
  const client = useClient();
  const [status, setStatus] = useState<PushStatus>(initialPushStatus);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (status !== "checking") return;
    let cancelled = false;
    subscriptionActive(client).then(
      (active) => {
        if (!cancelled) setStatus(active ? "enabled" : "off");
      },
      () => {
        if (!cancelled) setStatus("off");
      },
    );
    return () => {
      cancelled = true;
    };
  }, [client, status]);

  // Permission is requested only from this explicit click, never on load.
  async function toggle() {
    setBusy(true);
    setError(null);
    try {
      if (status === "enabled") {
        await disableNotifications(client);
        setStatus("off");
      } else {
        await enableNotifications(client);
        setStatus("enabled");
      }
    } catch (cause) {
      setError(cause instanceof Error && !("code" in cause) ? cause.message : errorMessage(cause));
      if (Notification.permission === "denied") setStatus("denied");
    } finally {
      setBusy(false);
    }
  }

  const text =
    status === "install"
      ? "On iPhone and iPad, notifications reach only the Home Screen app. Tap Share, choose Add to Home Screen, then open openside from the Home Screen and enable notifications here."
      : status === "unavailable"
        ? "This browser cannot receive push notifications here. Open openside over HTTPS in a browser with Web Push."
        : status === "denied"
          ? "Notifications are blocked for openside. Allow them in this browser's site settings, then come back here."
          : "Get notified when a task needs you or finishes while openside is in the background. Tap a notification to open its conversation.";

  return (
    <section aria-label="Notifications">
      <h3 className="settings-heading">Notifications</h3>
      <div className="settings-group">
        <StatusRow label="Response notifications">{PUSH_LABEL[status]}</StatusRow>
      </div>
      <p className="settings-text">{text}</p>
      {status === "off" || status === "enabled" || status === "checking" ? (
        <div className="settings-actions-row">
          <Button disabled={busy || status === "checking"} onClick={() => void toggle()}>
            {busy ? "Updating…" : status === "enabled" ? "Turn off notifications" : "Enable notifications"}
          </Button>
        </div>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
    </section>
  );
}

function Connection() {
  const client = useClient();
  const [health, setHealth] = useState<Health | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const check = async () => {
      try {
        const next = await client.health();
        if (cancelled) return;
        setHealth(next);
        setFailed(false);
      } catch {
        if (cancelled) return;
        setFailed(true);
      }
      timer = window.setTimeout(() => void check(), HEALTH_INTERVAL_MS);
    };
    void check();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client]);

  const daemon = failed ? "Unavailable" : health ? (health.ok ? "Connected" : "Degraded") : "Checking…";
  return (
    <section aria-label="Connection">
      <h3 className="settings-heading">Connection</h3>
      <div className="settings-group">
        <StatusRow label="Daemon">
          {health || failed ? <Badge tone={failed ? "danger" : health?.ok ? "success" : "attention"}>{daemon}</Badge> : daemon}
        </StatusRow>
        {health ? <StatusRow label="API version">{health.api_version}</StatusRow> : null}
      </div>
    </section>
  );
}

/** Content of the Settings dialog (aside-relay `Settings.tsx`). */
export function SettingsPage({ onWorkspace, onGuide, onSignOut }: { onWorkspace: () => void; onGuide: () => void; onSignOut: () => void }) {
  return (
    <div className="manage-body" data-section="settings">
      <button type="button" className="workspace-entry" onClick={onWorkspace}>
        <span className="tile-icon" aria-hidden="true">
          <SlidersHorizontal size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Workspace</strong>
          <span>Skills, routines, memory, accounts, vault, devices and configuration</span>
        </span>
        <ChevronRight size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <button type="button" className="workspace-entry" onClick={onGuide}>
        <span className="tile-icon" aria-hidden="true">
          <Compass size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Setup guide</strong>
          <span>Connect agents, import browser data, set up the vault and pair a phone</span>
        </span>
        <ChevronRight size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <Connection />
      <Notifications />
      <section aria-label="Appearance">
        <h3 className="settings-heading">Appearance</h3>
        <AppearanceSettings />
      </section>
      <section aria-label="Access">
        <h3 className="settings-heading">Access</h3>
        <div className="settings-group">
          <div className="status-row">
            <span>This browser</span>
            <Button variant="danger" size="compact" onClick={onSignOut}>
              Sign out
            </Button>
          </div>
        </div>
        <p className="settings-text">
          Signed in with a paired device token. Signing out removes the token from this browser; the device stays paired until
          you revoke it under Devices.
        </p>
      </section>
    </div>
  );
}
