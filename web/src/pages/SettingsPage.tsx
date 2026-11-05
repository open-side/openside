import { ChevronRight, Compass, Cpu, MessagesSquare, SlidersHorizontal, Smartphone, UserRound, type LucideIcon } from "lucide-react";
import { useEffect, useState } from "react";
import type { Health } from "../api/types";
import { AppearanceSettings } from "../components/AppearanceSettings";
import { useClient } from "../context";
import { desktopActive, desktopRequested, extensionAvailable } from "../desktopNotify";
import { describeFailure, disableNotifications, enableNotifications, pushSupported, storedSubscription, subscriptionActive } from "../push";
import { Button, cx, ICON_STROKE } from "../ui";
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

/** Panes of the Settings dialog; each keeps its own route. */
export type SettingsPane = "settings" | "models" | "channels" | "remote";

const NAV: { pane: SettingsPane | "workspace"; path: string; title: string; detail: string; Icon: LucideIcon }[] = [
  { pane: "settings", path: "/settings", title: "General", detail: "Connection, notifications, appearance and sign-out", Icon: UserRound },
  { pane: "models", path: "/models", title: "Models & accounts", detail: "Connect providers, add API keys and choose the fast and reasoning models", Icon: Cpu },
  { pane: "channels", path: "/channels", title: "Channels", detail: "Telegram, Slack and Discord accounts, and who answers on each", Icon: MessagesSquare },
  { pane: "remote", path: "/remote", title: "Remote access", detail: "Reach openside from your phone and connect it with a QR code", Icon: Smartphone },
  { pane: "workspace", path: "/workspace", title: "Workspace", detail: "Skills, routines, memory, vault, devices and configuration", Icon: SlidersHorizontal },
];

function NavRow({ title, detail, Icon, current, className, onClick }: { title: string; detail: string; Icon: LucideIcon; current?: boolean; className?: string; onClick: () => void }) {
  return (
    <button type="button" className={cx("settings-nav-row", className)} aria-current={current ? "page" : undefined} onClick={onClick}>
      <Icon className="settings-nav-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span className="settings-nav-copy">
        <strong>{title}</strong>
        <span className="settings-nav-detail">{detail}</span>
      </span>
      <ChevronRight className="settings-nav-chevron" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </button>
  );
}

/**
 * The Settings panes: a rail beside the pane on wide screens, a grouped list of rows that drill
 * into each pane on phones (aside-relay `Settings.tsx`). The phone list hides General, which is
 * the screen it sits on.
 */
export function SettingsNav({ current, onOpen, onGuide }: { current: SettingsPane; onOpen: (path: string) => void; onGuide: () => void }) {
  return (
    <nav className="settings-nav" aria-label="Settings sections">
      {NAV.map(({ pane, path, title, detail, Icon }) => (
        <NavRow key={pane} title={title} detail={detail} Icon={Icon} current={pane === current} className={pane === "settings" ? "settings-nav-general" : undefined} onClick={() => onOpen(path)} />
      ))}
      <NavRow title="Setup guide" detail="Connect agents, import browser data, set up the vault and pair a phone" Icon={Compass} className="workspace-entry" onClick={onGuide} />
    </nav>
  );
}

/** The General pane of the Settings dialog. */
export function SettingsPage({ onSignOut }: { onSignOut: () => void }) {
  return (
    <div className="manage-body" data-section="settings">
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
