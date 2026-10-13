import type { ReactNode } from "react";

/** A terminal equivalent of something the screen already does: secondary, never the only way. */
export function CliHint({ command, children }: { command: string; children?: ReactNode }) {
  return (
    <p className="cli-hint">
      {children ?? "Or from a terminal:"} <code>{command}</code>
    </p>
  );
}
