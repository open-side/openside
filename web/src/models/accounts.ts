import type { AccountUsageWindow, ModelAccountRef, ModelProvider, ProviderKeyInfo, SubscriptionAccount } from "../api/types";

/** The sources a person manages in the UI; the rest (broker, env, command, builtin) are configured in config.toml. */
export const SUBSCRIPTION_SOURCES = ["claude-code", "codex-cli"] as const;
export type SubscriptionSource = (typeof SUBSCRIPTION_SOURCES)[number];

export type AccountKind = "api-key" | SubscriptionSource | "other";

export interface AccountRow {
  ref: ModelAccountRef;
  provider: string;
  kind: AccountKind;
  label: string;
  /** Switched on by the person; `ready` and `reason` say whether it can serve requests now. */
  enabled: boolean;
  ready: boolean;
  reason: string;
  /** Position in the provider's rotation, or `null` when the account is not in it. */
  poolIndex: number | null;
  usage: AccountUsageWindow[];
  blockedUntilMs: number | null;
  key?: ProviderKeyInfo;
  subscription?: SubscriptionAccount;
}

export const refKey = (ref: ModelAccountRef) => `${ref.source}\u0000${ref.account_id}`;
export const sameRef = (a: ModelAccountRef, b: ModelAccountRef) => a.source === b.source && a.account_id === b.account_id;

function kindOf(source: string): AccountKind {
  return source === "vault" ? "api-key" : (SUBSCRIPTION_SOURCES as readonly string[]).includes(source) ? (source as SubscriptionSource) : "other";
}

interface Candidate {
  source: string;
  account_id: string;
  available: boolean;
  reason: string;
  label?: string | null;
  enabled?: boolean | null;
  ready?: boolean | null;
  in_pool?: boolean | null;
  usage_windows?: AccountUsageWindow[] | null;
  blocked_until_ms?: number | null;
}

/**
 * Every account that can serve one provider, merged from three views of the same thing: the
 * catalog's candidates (all sources, safe for any device), the Admin-only key list and the
 * Admin-only subscription list (which add last four, login command and reserve). Keys and
 * subscription accounts the catalog has not listed yet still appear.
 */
export function accountsOf(provider: ModelProvider, pool: ModelAccountRef[], keys: ProviderKeyInfo[], subscriptions: SubscriptionAccount[]): AccountRow[] {
  const rows = new Map<string, AccountRow>();
  const poolIndex = (ref: ModelAccountRef) => {
    const index = pool.findIndex((member) => sameRef(member, ref));
    return index < 0 ? null : index;
  };
  const row = (ref: ModelAccountRef, patch: Partial<AccountRow>): AccountRow => ({
    ref,
    provider: provider.provider,
    kind: kindOf(ref.source),
    label: ref.account_id,
    enabled: true,
    ready: false,
    reason: "",
    poolIndex: poolIndex(ref),
    usage: [],
    blockedUntilMs: null,
    ...patch,
  });

  for (const model of provider.models) {
    for (const candidate of model.accounts as Candidate[]) {
      const ref = { source: candidate.source, account_id: candidate.account_id };
      const existing = rows.get(refKey(ref));
      // The same account lists once per model; keep the most informative reading.
      if (existing) {
        existing.ready ||= candidate.ready ?? candidate.available;
        if (!existing.usage.length && candidate.usage_windows?.length) existing.usage = candidate.usage_windows;
        continue;
      }
      rows.set(
        refKey(ref),
        row(ref, {
          label: candidate.label || candidate.account_id,
          enabled: candidate.enabled ?? true,
          ready: candidate.ready ?? candidate.available,
          reason: candidate.reason,
          usage: candidate.usage_windows ?? [],
          blockedUntilMs: candidate.blocked_until_ms ?? null,
        }),
      );
    }
  }

  for (const key of keys.filter((candidate) => candidate.provider === provider.provider)) {
    const ref = { source: "vault", account_id: key.key_id };
    const base = rows.get(refKey(ref));
    rows.set(refKey(ref), { ...(base ?? row(ref, { ready: key.enabled })), key, kind: "api-key", label: key.label, enabled: key.enabled });
  }
  for (const account of subscriptions.filter((candidate) => candidate.provider === provider.provider)) {
    const ref = { source: account.source, account_id: account.account_id };
    const base = rows.get(refKey(ref));
    rows.set(refKey(ref), {
      ...(base ?? row(ref, {})),
      subscription: account,
      kind: kindOf(account.source),
      label: account.label,
      enabled: account.enabled,
      ready: account.available,
      reason: account.reason,
    });
  }

  // Rotation order first, then the accounts that are not in it.
  return [...rows.values()].sort((a, b) => (a.poolIndex ?? Infinity) - (b.poolIndex ?? Infinity));
}
