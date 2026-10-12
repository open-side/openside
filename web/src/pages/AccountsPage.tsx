import { RefreshCw, UserRound } from "lucide-react";
import { useCallback } from "react";
import type { AccountStatus, AccountUsageWindow } from "../api/types";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { Button, ICON_STROKE } from "../ui";
import { Badge, EmptyState, ResourceCard, ResourceView, SectionList } from "./blocks";
import { formatDateTime, formatRelative } from "./format";

function UsageBar({ window: usage }: { window: AccountUsageWindow }) {
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
          data-level={level}
          role="meter"
          aria-label={`${usage.name} usage`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
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

function Account({ account }: { account: AccountStatus }) {
  const blocked = account.blocked_until_ms != null && account.blocked_until_ms > Date.now();
  return (
    <ResourceCard
      data-account={account.id}
      title={account.label}
      detail={`${account.provider} · ${account.models.length ? account.models.join(", ") : "no models configured"}`}
      badge={blocked ? `Rate limited until ${formatDateTime(account.blocked_until_ms ?? 0)}` : undefined}
      badgeTone="attention"
    >
      {account.usage_windows.map((usage) => (
        <UsageBar key={usage.name} window={usage} />
      ))}
    </ResourceCard>
  );
}

/** Model accounts per credential source, with usage windows and rate-limit state. */
export function AccountsPage() {
  const client = useClient();
  const load = useCallback(() => client.accounts(), [client]);
  const accounts = useResource(load);
  const total = accounts.data?.sources.reduce((sum, source) => sum + source.accounts.length, 0) ?? 0;

  return (
    <div className="manage-body" data-section="accounts">
      <SectionList
        label="Accounts and usage"
        summary={accounts.data?.sources.length ? `${total} ${total === 1 ? "account" : "accounts"} · usage as reported by each source` : undefined}
        actions={
          accounts.forbidden ? undefined : (
            <Button size="compact" onClick={() => void accounts.reload()}>
              <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Refresh
            </Button>
          )
        }
      >
        <ResourceView resource={accounts} what="model accounts">
          {(data) =>
            data.sources.length === 0 ? (
              <EmptyState icon={UserRound} title="No credential sources" detail="Add model accounts to config.toml on the computer running openside." />
            ) : null
          }
        </ResourceView>
      </SectionList>
      {accounts.forbidden
        ? null
        : accounts.data?.sources.map((source) => (
            <section key={source.name} className="section-list" aria-label={`Source ${source.name}`} data-source={source.name}>
              <div className="section-list-header">
                <h3 className="section-list-title">{source.name}</h3>
                <div className="section-list-actions">
                  <Badge>{source.kind}</Badge>
                  <Badge tone={source.available ? "success" : "attention"}>{source.available ? "Available" : "Unavailable"}</Badge>
                </div>
              </div>
              {source.reason ? <p className="section-list-description">{source.reason}</p> : null}
              {source.accounts.length ? (
                <ul className="resource-list" aria-label={`Accounts of ${source.name}`}>
                  {source.accounts.map((account) => (
                    <li key={account.id}>
                      <Account account={account} />
                    </li>
                  ))}
                </ul>
              ) : null}
            </section>
          ))}
      {accounts.data?.truncated ? <p className="settings-text">Some accounts are not shown.</p> : null}
    </div>
  );
}
