import { useEffect, useState } from "react";
import type { ComputerUseStatus } from "../api/types";
import { useClient } from "../context";
import { t } from "../i18n";
import { SettingsCard, SettingsRow, SettingsSection, Switch, useToast } from "../ui";

/** Settings › Agents › Computer use: the switch, and what this computer supports. */
export function ComputerUseSettings() {
  const client = useClient();
  const toast = useToast();
  const [status, setStatus] = useState<ComputerUseStatus | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let live = true;
    client.computerUse().then(
      (next) => live && setStatus(next),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client]);

  const toggle = (on: boolean) => {
    setSaving(true);
    client
      .updatePreferences({ computer_use: on })
      .then(
        (preferences) => setStatus((current) => current && { ...current, enabled: preferences.computer_use }),
        (error: Error) => toast.notify(error.message, "error"),
      )
      .finally(() => setSaving(false));
  };

  const note = !status
    ? t("settings.status.checking")
    : status.eligible
      ? status.reasons.length
        ? t("settings.computerUse.partial", { reasons: status.reasons.join(" ") })
        : t("settings.computerUse.description")
      : t("settings.computerUse.unsupported", { reasons: status.reasons.join(" ") });

  return (
    <SettingsSection label={t("settings.computerUse.title")}>
      <SettingsCard>
        <SettingsRow
          title={t("settings.computerUse.row")}
          description={note}
          control={({ labelId, descriptionId }) => (
            <Switch
              checked={!!status?.enabled}
              disabled={!status || saving || (!status.eligible && !status.enabled)}
              onChange={toggle}
              aria-labelledby={labelId}
              aria-describedby={descriptionId}
            />
          )}
        />
      </SettingsCard>
    </SettingsSection>
  );
}
