import type { ReactNode } from "react";
import { t } from "../i18n";

/** A terminal equivalent of something the screen already does: secondary, never the only way. */
export function CliHint({ command, children }: { command: string; children?: ReactNode }) {
  return (
    <p className="cli-hint">
      {children ?? t("settings.cliHint")} <code>{command}</code>
    </p>
  );
}
