import type { AccountUsageWindow } from "../api/types";
import { formatDateTime, formatRelative } from "../pages/format";

export function UsageBar({ window: usage }: { window: AccountUsageWindow }) {
  const percent = usage.used_percent;
  const level = percent == null ? undefined : percent >= 90 ? "full" : percent >= 70 ? "high" : "normal";
  const reset = usage.reset_at_ms ? `resets ${formatRelative(usage.reset_at_ms)}` : "";
  return (
    <div className="usage" data-window={usage.name}>
      <span className="usage-name">{usage.name}</span>
      {percent == null ? (
        <span className="status-row-value">usage unknown</span>
      ) : (
        <span
          className="usage-bar"
          role="meter"
          aria-label={`${usage.name} usage`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          data-level={level}
        >
          <span style={{ width: `${Math.min(100, Math.max(0, percent))}%` }} />
        </span>
      )}
      {percent != null || reset ? (
        <span className="usage-text" title={usage.reset_at_ms ? formatDateTime(usage.reset_at_ms) : undefined}>
          {[percent == null ? "" : `${Math.round(percent)}% used`, reset].filter(Boolean).join(" · ")}
        </span>
      ) : null}
    </div>
  );
}
