import { ArrowDown, ArrowUp, KeyRound, Plus, Terminal, UserRound, X } from "lucide-react";
import { useState } from "react";
import type { ModelAccountRef, ModelProvider } from "../api/types";
import { Badge, Disclosure, ResourceCard } from "../pages/blocks";
import { formatDateTime } from "../pages/format";
import { Button, IconButton, ICON_STROKE, Select, Switch } from "../ui";
import { refKey, type AccountRow } from "./accounts";
import { KEY_PROVIDERS, PROVIDER_SUBSCRIPTION, providerLabel, providerReady, SUBSCRIPTION_LABELS } from "./providers";
import { UsageBar } from "./Usage";
import "./models.css";

export interface ProviderCardProps {
  provider: ModelProvider;
  rows: AccountRow[];
  pool: ModelAccountRef[];
  /** Limited device: status only, no account, rotation or default controls. */
  readOnly: boolean;
  /** First-run step: accounts only, no rotation or model list. */
  compact?: boolean;
  /** Key of the row (or `pool`) with a request in flight. */
  busy: string | null;
  onAddKey: (provider: string) => void;
  onAddSubscription: (source: string) => void;
  onSignIn: (row: AccountRow) => void;
  onManage: (row: AccountRow) => void;
  onToggle: (row: AccountRow, enabled: boolean) => void;
  onPool: (provider: string, accounts: ModelAccountRef[]) => void;
}

const COLLAPSED_MODELS = 4;

function statusOf(row: AccountRow): { text: string; tone: "success" | "attention" | "neutral" } {
  if (!row.enabled) return { text: "Off", tone: "neutral" };
  if (row.blockedUntilMs != null && row.blockedUntilMs > Date.now()) return { text: `Rate limited until ${formatDateTime(row.blockedUntilMs)}`, tone: "attention" };
  if (row.subscription && !row.subscription.installed) return { text: "Not installed", tone: "neutral" };
  if (row.subscription && !row.subscription.logged_in) return { text: "Not signed in", tone: "neutral" };
  return row.ready ? { text: "Ready", tone: "success" } : { text: "Unavailable", tone: "attention" };
}

function detailOf(row: AccountRow) {
  const rotation = row.poolIndex === null ? " · Not in rotation" : "";
  if (row.key) {
    return (
      <>
        <span className="key-mask" aria-label={`Key ending in ${row.key.last_four}`}>
          ••••••••{row.key.last_four}
        </span>{" "}
        · added {formatDateTime(row.key.created_at)}
        {rotation}
      </>
    );
  }
  if (row.subscription) {
    const name = SUBSCRIPTION_LABELS[row.subscription.source as keyof typeof SUBSCRIPTION_LABELS] ?? row.subscription.source;
    // The status badge already says "Not signed in" / "Not installed"; repeating the reason adds noise.
    return `${name}${row.subscription.managed ? "" : " · your terminal's sign-in"}${rotation}`;
  }
  return `Source ${row.ref.source}${rotation}${row.reason && !row.ready ? ` · ${row.reason}` : ""}`;
}

function AccountRowView({ row, readOnly, busy, onSignIn, onManage, onToggle }: { row: AccountRow; readOnly: boolean; busy: boolean } & Pick<ProviderCardProps, "onSignIn" | "onManage" | "onToggle">) {
  const status = statusOf(row);
  const needsSignIn = row.subscription && row.subscription.managed && !row.subscription.logged_in;
  const Icon = row.kind === "api-key" ? KeyRound : row.kind === "other" ? UserRound : Terminal;
  const editable = !readOnly && (row.key || row.subscription);
  return (
    <ResourceCard
      icon={Icon}
      data-account={row.ref.account_id}
      data-source={row.ref.source}
      title={row.label}
      badge={status.text}
      badgeTone={status.tone}
      detail={detailOf(row)}
      trailing={
        editable ? (
          <span className="row-actions">
            {needsSignIn ? (
              <Button size="compact" variant="primary" disabled={busy} aria-label={`Sign in ${row.label}`} onClick={() => onSignIn(row)}>
                Sign in
              </Button>
            ) : (
              <Switch checked={row.enabled} disabled={busy} label={`Use ${row.label}`} onChange={(enabled) => onToggle(row, enabled)} />
            )}
            <Button size="compact" variant="ghost" disabled={busy} aria-label={`Manage ${row.label}`} onClick={() => onManage(row)}>
              Manage
            </Button>
          </span>
        ) : undefined
      }
    >
      {row.usage.map((usage) => (
        <UsageBar key={usage.name} window={usage} />
      ))}
    </ResourceCard>
  );
}

/** Which accounts rotate for this provider, and in what order. */
function Rotation({ provider, rows, pool, readOnly, busy, onPool }: { provider: string; rows: AccountRow[]; pool: ModelAccountRef[]; readOnly: boolean; busy: boolean; onPool: ProviderCardProps["onPool"] }) {
  const byKey = new Map(rows.map((row) => [refKey(row.ref), row]));
  const members = pool.map((ref) => byKey.get(refKey(ref))).filter((row): row is AccountRow => row !== undefined);
  const outside = rows.filter((row) => row.poolIndex === null);
  const move = (from: number, to: number) => {
    const next = [...pool];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item!);
    onPool(provider, next);
  };
  return (
    <div className="rotation" data-rotation={provider}>
      <h4 className="rotation-title">Rotation</h4>
      <p className="rotation-help">A conversation stays on one account. When it hits a limit or its sign-in stops working, openside moves to the next one in this list.</p>
      {members.length === 0 ? (
        <p className="rotation-empty" role="status">
          No account is in the rotation, so {providerLabel(provider)} cannot be used.
        </p>
      ) : (
        <ol className="rotation-list" aria-label={`${providerLabel(provider)} rotation order`}>
          {members.map((row, index) => (
            <li key={refKey(row.ref)}>
              <span className="rotation-name">{row.label}</span>
              {readOnly ? null : (
                <span className="rotation-actions">
                  <IconButton label={`Move ${row.label} up`} size="small" disabled={busy || index === 0} onClick={() => move(index, index - 1)}>
                    <ArrowUp size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                  <IconButton label={`Move ${row.label} down`} size="small" disabled={busy || index === members.length - 1} onClick={() => move(index, index + 1)}>
                    <ArrowDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                  <IconButton label={`Remove ${row.label} from the rotation`} size="small" disabled={busy} onClick={() => onPool(provider, pool.filter((ref) => refKey(ref) !== refKey(row.ref)))}>
                    <X size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                </span>
              )}
            </li>
          ))}
        </ol>
      )}
      {readOnly || outside.length === 0 ? null : (
        <div className="rotation-add">
          <label id={`rotation-add-${provider}`}>Add to rotation</label>
          <Select
            aria-label={`Add an account to the ${providerLabel(provider)} rotation`}
            value=""
            placeholder="Choose an account…"
            disabled={busy}
            onChange={(value) => {
              const picked = outside.find((row) => refKey(row.ref) === value);
              if (picked) onPool(provider, [...pool, picked.ref]);
            }}
            options={outside.map((row) => ({ value: refKey(row.ref), label: row.label, detail: statusOf(row).text }))}
          />
        </div>
      )}
    </div>
  );
}

function ModelRows({ provider }: { provider: ModelProvider }) {
  const [all, setAll] = useState(false);
  const shown = all ? provider.models : provider.models.slice(0, COLLAPSED_MODELS);
  return (
    <div className="model-rows">
      <ul aria-label={`${providerLabel(provider.provider)} models`}>
        {shown.map((model) => (
          <li key={model.id} data-available={model.available} data-model={model.id}>
            <span className="model-row-name">
              <strong>{model.label}</strong>
              <code>{model.id}</code>
            </span>
            {model.available ? null : <Badge>Unavailable</Badge>}
            {model.available || !model.reason ? null : <span className="model-row-reason">{model.reason}</span>}
          </li>
        ))}
      </ul>
      {provider.models.length > COLLAPSED_MODELS ? (
        <Disclosure expanded={all} onToggle={() => setAll((current) => !current)}>
          {all ? "Show fewer models" : `Show all ${provider.models.length} models`}
        </Disclosure>
      ) : null}
    </div>
  );
}

/** One provider: its accounts (keys, subscriptions, broker), how they rotate, and the models that gives. */
export function ProviderCard(props: ProviderCardProps) {
  const { provider, rows, pool, readOnly, compact, busy } = props;
  const name = providerLabel(provider.provider);
  const ready = providerReady(provider);
  const subscription = PROVIDER_SUBSCRIPTION[provider.provider];
  return (
    <section className="section-list provider" aria-label={`Provider ${name}`} data-provider={provider.provider}>
      <div className="section-list-header">
        <h3 className="section-list-title">{name}</h3>
        <div className="section-list-actions">
          <Badge tone={ready ? "success" : "neutral"}>{ready ? "Ready" : "Not connected"}</Badge>
          {readOnly ? null : (
            <>
              {KEY_PROVIDERS.has(provider.provider) ? (
                <Button size="compact" variant={rows.length === 0 ? "primary" : "secondary"} aria-label={`Add ${name} API key`} onClick={() => props.onAddKey(provider.provider)}>
                  <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  API key
                </Button>
              ) : null}
              {subscription ? (
                <Button size="compact" aria-label={`Add ${SUBSCRIPTION_LABELS[subscription]} account`} onClick={() => props.onAddSubscription(subscription)}>
                  <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  {SUBSCRIPTION_LABELS[subscription]}
                </Button>
              ) : null}
            </>
          )}
        </div>
      </div>
      {rows.length === 0 ? (
        <p className="settings-text accounts-empty">{readOnly ? "No account is connected for this provider." : `No account yet. Add an API key${subscription ? ` or a ${SUBSCRIPTION_LABELS[subscription]} account` : ""} to use ${name} models.`}</p>
      ) : (
        <ul className="resource-list" aria-label={`Accounts for ${name}`}>
          {rows.map((row) => (
            <li key={refKey(row.ref)}>
              <AccountRowView row={row} readOnly={readOnly} busy={busy === refKey(row.ref)} onSignIn={props.onSignIn} onManage={props.onManage} onToggle={props.onToggle} />
            </li>
          ))}
        </ul>
      )}
      {compact || rows.length === 0 ? null : <Rotation provider={provider.provider} rows={rows} pool={pool} readOnly={readOnly} busy={busy === `pool:${provider.provider}`} onPool={props.onPool} />}
      {compact ? null : <ModelRows provider={provider} />}
    </section>
  );
}
