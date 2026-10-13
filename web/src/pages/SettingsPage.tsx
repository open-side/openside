import { ChevronRight, Compass, Cpu, MessagesSquare, SlidersHorizontal, Smartphone } from "lucide-react";
import { useEffect, useState } from "react";
import type { Health } from "../api/types";
import { AppearanceSettings } from "../components/AppearanceSettings";
import { useClient } from "../context";
import { desktopActive, desktopRequested, extensionAvailable } from "../desktopNotify";
import { describeFailure, disableNotifications, enableNotifications, pushSupported, storedSubscription, subscriptionActive } from "../push";
import { Button, ICON_STROKE } from "../ui";
import { Badge, ErrorNote, StatusRow } from "./blocks";

/** aside-relay re-checks the connection every five seconds while Settings is open. */
const HEALTH_INTERVAL_MS = 5_000;

type PushStatus = "checking" | "enabled" | "desktop" | "off" | "denied" | "unavailable" | "install";

const PUSH_LABEL: Record<PushStatus, string> = {
  checking: "Checking…",
  enabled: "Enabled",
  desktop: "Enabled (desktop)",
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
  if (desktopRequested()) return "checking";
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
    const settle = (next: PushStatus) => {
      if (!cancelled) setStatus(next);
    };
    if (desktopRequested()) {
      void desktopActive().then((active) => settle(active ? "desktop" : "off"));
    } else {
      subscriptionActive(client).then(
        (active) => settle(active ? "enabled" : "off"),
        () => settle("off"),
      );
    }
    return () => {
      cancelled = true;
    };
  }, [client, status]);

  // Web Push may be missing or blocked while the extension's desktop notifications still work.
  useEffect(() => {
    if (status !== "unavailable" && status !== "denied") return;
    let cancelled = false;
    void extensionAvailable().then((found) => {
      if (found && !cancelled) setStatus("off");
    });
    return () => {
      cancelled = true;
    };
  }, [status]);

  // Permission is requested only from this explicit click, never on load.
  async function toggle() {
    setBusy(true);
    setError(null);
    try {
      if (status === "enabled" || status === "desktop") {
        await disableNotifications(client);
        setStatus("off");
      } else {
        setStatus((await enableNotifications(client)) === "desktop" ? "desktop" : "enabled");
      }
    } catch (cause) {
      setError(describeFailure(cause));
      if ("Notification" in window && Notification.permission === "denied") setStatus("denied");
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
          : status === "desktop"
            ? "Using desktop notifications from the openside extension instead of Web Push. You get a notification when a task finishes or needs you, even with every openside tab closed. The conversation you are looking at stays quiet."
            : "Get notified when a task needs you or finishes while openside is in the background. Tap a notification to open its conversation.";

  return (
    <section aria-label="Notifications">
      <h3 className="settings-heading">Notifications</h3>
      <div className="settings-group">
        <StatusRow label="Response notifications">{PUSH_LABEL[status]}</StatusRow>
      </div>
      <p className="settings-text">{text}</p>
      {status === "off" || status === "enabled" || status === "desktop" || status === "checking" ? (
        <div className="settings-actions-row">
          <Button disabled={busy || status === "checking"} onClick={() => void toggle()}>
            {busy ? "Updating…" : status === "enabled" || status === "desktop" ? "Turn off notifications" : "Enable notifications"}
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
export function SettingsPage({ onModels, onChannels, onRemote, onWorkspace, onGuide, onSignOut }: { onModels: () => void; onChannels: () => void; onRemote: () => void; onWorkspace: () => void; onGuide: () => void; onSignOut: () => void }) {
  return (
    <div className="manage-body" data-section="settings">
      <button type="button" className="workspace-entry" onClick={onModels}>
        <span className="tile-icon" aria-hidden="true">
          <Cpu size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Models &amp; accounts</strong>
          <span>Connect providers, add API keys and choose the fast and reasoning models</span>
        </span>
        <ChevronRight size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <button type="button" className="workspace-entry" onClick={onChannels}>
        <span className="tile-icon" aria-hidden="true">
          <MessagesSquare size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Channels</strong>
          <span>Telegram, Slack and Discord accounts, and who answers on each</span>
        </span>
        <ChevronRight size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <button type="button" className="workspace-entry" onClick={onRemote}>
        <span className="tile-icon" aria-hidden="true">
          <Smartphone size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Remote access</strong>
          <span>Reach openside from your phone and connect it with a QR code</span>
        </span>
        <ChevronRight size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <button type="button" className="workspace-entry" onClick={onWorkspace}>
        <span className="tile-icon" aria-hidden="true">
          <SlidersHorizontal size={20} strokeWidth={ICON_STROKE} />
        </span>
        <span className="tile-copy">
          <strong>Workspace</strong>
          <span>Skills, routines, memory, vault, devices and configuration</span>
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
