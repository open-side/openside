import { useEffect, useState } from "react";
import { useClient } from "../../context";
import { desktopActive, desktopRequested, extensionAvailable } from "../../desktopNotify";
import { t, type MessageKey } from "../../i18n";
import { describeFailure, disableNotifications, enableNotifications, pushSupported, storedSubscription, subscriptionActive } from "../../push";
import { Button, SettingsRow } from "../../ui";

type PushStatus = "checking" | "enabled" | "desktop" | "off" | "denied" | "unavailable" | "install";

const PUSH_LABEL: Record<PushStatus, MessageKey> = {
  checking: "settings.status.checking",
  enabled: "common.enabled",
  desktop: "settings.notifications.status.desktop",
  off: "common.off",
  denied: "settings.notifications.status.denied",
  unavailable: "settings.status.unavailable",
  install: "settings.notifications.status.install",
};

/** iOS delivers Web Push only to Home Screen apps; a Safari tab has no PushManager at all. */
function iosBrowserTab(): boolean {
  const ios = /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  const standalone =
    (typeof window.matchMedia === "function" && window.matchMedia("(display-mode: standalone)").matches) ||
    ("standalone" in navigator && navigator.standalone === true);
  return ios && !standalone;
}

function initialPushStatus(): PushStatus {
  if (iosBrowserTab()) return "install";
  if (desktopRequested()) return "checking";
  if (!pushSupported()) return "unavailable";
  if (Notification.permission === "denied") return "denied";
  return storedSubscription() ? "checking" : "off";
}

/**
 * Task notifications pick *what* the daemon sends; this row is whether *this browser* receives
 * them: a Web Push subscription, or the openside extension where Web Push has no push service.
 * Aside is a desktop app with OS notifications and needs no such row.
 */
export function DeviceNotificationsRow() {
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

  const text = t(
    status === "install"
      ? "settings.notifications.text.install"
      : status === "unavailable"
        ? "settings.notifications.text.unavailable"
        : status === "denied"
          ? "settings.notifications.text.denied"
          : status === "desktop"
            ? "settings.notifications.text.desktop"
            : "settings.notifications.text.off",
  );
  const on = status === "enabled" || status === "desktop";
  const actionable = status === "off" || on || status === "checking";
  return (
    <SettingsRow
      title={t("agentSettings.notifications.device")}
      description={
        <>
          {text}
          {error ? (
            <span className="agents-error" role="alert">
              {error}
            </span>
          ) : null}
        </>
      }
      control={
        <>
          <span className="agents-status" data-on={on ? "" : undefined}>
            {t(PUSH_LABEL[status])}
          </span>
          {actionable ? (
            <Button disabled={busy || status === "checking"} onClick={() => void toggle()}>
              {busy ? t("settings.notifications.updating") : on ? t("settings.notifications.turnOff") : t("settings.notifications.enable")}
            </Button>
          ) : null}
        </>
      }
    />
  );
}
