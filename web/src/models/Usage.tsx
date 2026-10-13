import type { AccountUsageWindow } from "../api/types";
import { formatDateTime, formatRelative } from "../pages/format";

/** One usage window as what is left of it: the bar and the text both count down from the limit. */
export function UsageBar({ window: usage }: { window: AccountUsageWindow }) {
  const left = usage.used_percent == null ? null : Math.min(100, Math.max(0, 100 - usage.used_percent));
  const level = left == null ? undefined : left <= 10 ? "full" : left <= 30 ? "high" : "normal";
  const reset = usage.reset_at_ms ? `resets ${formatRelative(usage.reset_at_ms)}` : "";
  return (
    <div className="usage" data-window={usage.name}>
      <span className="usage-name">{usage.name}</span>
      {left == null ? (
        <span className="status-row-value">usage unknown</span>
      ) : (
        <span className="usage-bar" role="meter" aria-label={`${usage.name} remaining`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={left} data-level={level}>
          <span style={{ width: `${left}%` }} />
        </span>
      )}
      {left != null || reset ? (
        <span className="usage-text" title={usage.reset_at_ms ? formatDateTime(usage.reset_at_ms) : undefined}>
          {[left == null ? "" : `${Math.round(left)}% left`, reset].filter(Boolean).join(" · ")}
        </span>
      ) : null}
    </div>
  );
}
