import type { AccountsResponse, ModelAccountRef, ModelCatalog, ProviderKeyInfo, SubscriptionAccount } from "../api/types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";

const NEEDS_CREDENTIAL = "No credential yet: add an API key or sign in to a subscription.";
const NOT_IN_POOL = "No account is in the rotation.";

interface Curated {
  id: string;
  label: string;
  recommended_for: "fast" | "reasoning" | null;
  tier: "fast" | "reasoning" | "frontier";
  strengths: string;
  in_catalog: boolean;
}

const CURATED: Record<string, Curated[]> = {
  anthropic: [
    { id: "anthropic:claude-sonnet-5-5", label: "Claude Sonnet 5.5", recommended_for: "reasoning", tier: "reasoning", strengths: "Multi-step research, coding", in_catalog: true },
    { id: "anthropic:claude-haiku-5-5", label: "Claude Haiku 5.5", recommended_for: "fast", tier: "fast", strengths: "Quick lookups, simple edits", in_catalog: true },
  ],
  openai: [
    { id: "openai:gpt-6.1-sol", label: "GPT-6.1 Sol", recommended_for: "reasoning", tier: "reasoning", strengths: "Multi-step research, coding", in_catalog: true },
    { id: "openai:gpt-6-astra", label: "GPT-6 Astra", recommended_for: "reasoning", tier: "frontier", strengths: "Hard reasoning, architecture", in_catalog: true },
    { id: "openai:gpt-6-luna", label: "GPT-6 Luna", recommended_for: "fast", tier: "fast", strengths: "Quick lookups, simple edits", in_catalog: true },
  ],
  google: [{ id: "google-gla:gemini-2.5-pro", label: "Gemini 2.5 Pro", recommended_for: null, tier: "reasoning", strengths: "", in_catalog: false }],
};

const SUBSCRIPTION_PROVIDER: Record<string, string> = { "claude-code": "anthropic", "codex-cli": "openai" };

export interface ModelState {
  /** False: a read-scope device (key and subscription lists and every write answer 403). */
  admin: boolean;
  keys: ProviderKeyInfo[];
  subscriptions: SubscriptionAccount[];
  /** Which subscription CLIs exist on the daemon's computer. */
  installed: Record<string, boolean>;
  pools: Record<string, ModelAccountRef[]>;
  defaultModel: string | null;
  /** Null: the fast role follows the reasoning model. */
  fastModel: string | null;
  fallbacks: string[];
  accounts: AccountsResponse;
  /** Every key value the daemon was sent, for "never rendered back" assertions. */
  received: string[];
  /** Answer key writes with 409 vault_unavailable. */
  vaultLocked: boolean;
  /** Sessions' preferred account, as the daemon last accepted it. */
  sessionAccounts: Record<string, ModelAccountRef | null>;
  /** Accounts of other sources (broker, env, ...) per provider, as the catalog lists them. */
  extra: Record<string, Partial<Candidate>[]>;
  /** Accounts whose next GET reports a finished sign-in. */
  signInLands: string[];
  nextId: number;
}

export const SECRET = "sk-test-SECRET-9f3a";

export function modelState(patch: Partial<ModelState> = {}): ModelState {
  return {
    admin: true,
    keys: [],
    subscriptions: [],
    installed: { "claude-code": true, "codex-cli": false },
    pools: {},
    defaultModel: null,
    fastModel: null,
    fallbacks: [],
    accounts: { sources: [], cached_for_ms: 0, truncated: false },
    received: [],
    vaultLocked: false,
    sessionAccounts: {},
    extra: {},
    signInLands: [],
    nextId: 1,
    ...patch,
  };
}

export function keyInfo(provider: string, keyId: string, label: string, lastFour = "9f3a", patch: Partial<ProviderKeyInfo> = {}): ProviderKeyInfo {
  return { provider, key_id: keyId, label, last_four: lastFour, created_at: "2026-10-12T09:30:00Z", enabled: true, reserve_percent: 10, ...patch };
}

export function subscriptionAccount(source: string, accountId: string, label: string, patch: Partial<SubscriptionAccount> = {}): SubscriptionAccount {
  return {
    source,
    provider: SUBSCRIPTION_PROVIDER[source]!,
    account_id: accountId,
    label,
    managed: true,
    installed: true,
    logged_in: true,
    enabled: true,
    available: true,
    reason: "",
    login_command: source === "claude-code" ? `CLAUDE_CONFIG_DIR=~/.local/share/openside/accounts/${accountId} claude auth login` : `CODEX_HOME=~/.local/share/openside/accounts/${accountId} codex login`,
    created_at: "2026-10-12T09:30:00Z",
    reserve_percent: 10,
    ...patch,
  };
}

export interface Candidate {
  source: string;
  account_id: string;
  label: string;
  enabled: boolean;
  ready: boolean;
  in_pool: boolean;
  available: boolean;
  reason: string;
  usage_windows: { name: string; used_percent: number | null; reset_at_ms: number | null }[];
  blocked_until_ms: number | null;
}

function candidates(state: ModelState, provider: string): Candidate[] {
  const pool = state.pools[provider] ?? [];
  const make = (source: string, accountId: string, label: string, enabled: boolean, ready: boolean, reason: string): Candidate => {
    const inPool = pool.some((ref) => ref.source === source && ref.account_id === accountId);
    return {
      source,
      account_id: accountId,
      label,
      enabled,
      ready,
      in_pool: inPool,
      available: enabled && ready && inPool,
      reason: !enabled ? "Switched off." : !ready ? reason : !inPool ? NOT_IN_POOL : "",
      usage_windows: [],
      blocked_until_ms: null,
    };
  };
  return [
    ...state.keys.filter((key) => key.provider === provider).map((key) => make("vault", key.key_id, key.label, key.enabled, true, "")),
    ...state.subscriptions.filter((sub) => sub.provider === provider).map((sub) => make(sub.source, sub.account_id, sub.label, sub.enabled, sub.logged_in, sub.reason || "Not signed in.")),
    ...(state.extra[provider] ?? []).map((extra) => ({ ...make(extra.source!, extra.account_id!, extra.label ?? extra.account_id!, true, true, ""), ...extra })),
  ];
}

function catalog(state: ModelState): ModelCatalog {
  return {
    providers: Object.entries(CURATED).map(([provider, models]) => {
      const rows = candidates(state, provider);
      const usable = rows.find((row) => row.available);
      return {
        provider,
        models: models.map((model) => ({
          ...model,
          available: usable !== undefined,
          reason: usable ? "" : rows.length ? (rows.find((row) => row.reason)?.reason ?? NEEDS_CREDENTIAL) : NEEDS_CREDENTIAL,
          source: usable?.source ?? null,
          account_id: usable?.account_id ?? null,
          accounts: rows,
        })),
        pool: state.pools[provider] ?? [],
      };
    }),
    default_model: state.defaultModel,
    fast_model: state.fastModel,
    fallback_models: state.fallbacks,
    cached_for_ms: 0,
    truncated: false,
  } as unknown as ModelCatalog;
}

const forbidden = () => json(403, { code: "forbidden", message: "needs admin" });
const absent = () => json(404, { code: "key_not_found", message: "none" });

/** The Models & accounts endpoints of the daemon; undefined for any other request. */
export function modelRoutes(state: ModelState): (call: Call) => Response | undefined {
  return (call) => {
    const path = new URL(call.path, "http://daemon.test").pathname;
    const key = `${call.method} ${path}`;
    const body = (call.body ?? {}) as Record<string, unknown>;
    if (key === "GET /v1/settings/models") return json(200, catalog(state));
    if (key === "GET /v1/accounts") return json(200, state.accounts);
    if (key === "GET /v1/settings/pools") return json(200, { pools: Object.entries(state.pools).map(([provider, accounts]) => ({ provider, accounts })) });
    if (!state.admin) {
      if (call.method !== "GET" || path === "/v1/settings/keys" || path.startsWith("/v1/settings/subscriptions")) return path.startsWith("/v1/settings") || call.method !== "GET" ? forbidden() : undefined;
    }
    if (key === "GET /v1/settings/keys") return json(200, { keys: state.keys });
    if (key === "GET /v1/settings/subscriptions") {
      return json(200, {
        sources: Object.entries(SUBSCRIPTION_PROVIDER).map(([source, provider]) => ({
          source,
          provider,
          installed: state.installed[source] ?? false,
          accounts: state.subscriptions.filter((sub) => sub.source === source),
        })),
      });
    }
    if (key === "PUT /v1/settings/models") {
      const request = call.body as { default_model: string; fast_model?: string | null; fallback_models: string[] };
      const all = catalog(state).providers.flatMap((provider) => provider.models);
      const unusable = [request.default_model, request.fast_model, ...request.fallback_models].find((id) => id && !all.some((model) => model.id === id && model.available));
      if (unusable) return json(400, { code: "model_unavailable", message: `${unusable} is unavailable` });
      state.defaultModel = request.default_model;
      // Like the daemon: a fast model equal to the reasoning model is stored as null.
      state.fastModel = request.fast_model && request.fast_model !== request.default_model ? request.fast_model : null;
      state.fallbacks = request.fallback_models;
      return json(200, { default_model: request.default_model, fast_model: state.fastModel, fallback_models: request.fallback_models, restart_required: false, effective: "new_sessions_and_home_next_turn" });
    }

    const pool = /^PUT \/v1\/settings\/pools\/([^/]+)$/.exec(key);
    if (pool) {
      state.pools[pool[1]!] = body.accounts as ModelAccountRef[];
      return json(200, { provider: pool[1], accounts: state.pools[pool[1]!] });
    }
    const session = /^PUT \/v1\/sessions\/([^/]+)\/account$/.exec(key);
    if (session) {
      state.sessionAccounts[session[1]!] = body.account as ModelAccountRef | null;
      return json(200, { session_id: session[1], provider: "anthropic", account: body.account, explicit: body.account !== null, effective: "next_request" });
    }

    const keys = /^(POST|PUT|PATCH|DELETE) \/v1\/settings\/keys\/([^/]+)(?:\/([^/]+))?$/.exec(key);
    if (keys) {
      if (state.vaultLocked) return json(409, { code: "vault_unavailable", message: "locked" });
      const [, method, provider, keyId] = keys as unknown as [string, string, string, string | undefined];
      if (method === "POST") {
        const value = String(body.key);
        state.received.push(value);
        const info = keyInfo(provider, `key-${state.nextId++}`, String(body.label), value.slice(-4));
        state.keys.push(info);
        return json(201, info);
      }
      const stored = state.keys.find((candidate) => candidate.provider === provider && candidate.key_id === keyId);
      if (!stored) return absent();
      if (method === "DELETE") {
        state.keys = state.keys.filter((candidate) => candidate !== stored);
        state.pools[provider] = (state.pools[provider] ?? []).filter((ref) => ref.account_id !== keyId);
        return new Response(null, { status: 204 });
      }
      if (method === "PUT") {
        const value = String(body.key);
        state.received.push(value);
        stored.last_four = value.slice(-4);
        if (typeof body.label === "string") stored.label = body.label;
      } else {
        if (typeof body.label === "string") stored.label = body.label;
        if (typeof body.enabled === "boolean") stored.enabled = body.enabled;
        if (typeof body.reserve_percent === "number") stored.reserve_percent = body.reserve_percent;
      }
      return json(200, stored);
    }

    const subs = /^(POST|GET|PATCH|DELETE) \/v1\/settings\/subscriptions\/([^/]+)(?:\/([^/]+))?$/.exec(key);
    if (subs) {
      const [, method, source, accountId] = subs as unknown as [string, string, string, string | undefined];
      if (!(source in SUBSCRIPTION_PROVIDER)) return json(400, { code: "invalid_source", message: "unknown" });
      if (method === "POST") {
        const created = subscriptionAccount(source, `acct-${state.nextId++}`, String(body.label), { logged_in: false, available: false, reason: "Not signed in.", installed: state.installed[source] ?? false });
        state.subscriptions.push(created);
        return json(201, created);
      }
      const stored = state.subscriptions.find((candidate) => candidate.source === source && candidate.account_id === accountId);
      if (!stored) return absent();
      if (method === "DELETE") {
        state.subscriptions = state.subscriptions.filter((candidate) => candidate !== stored);
        state.pools[stored.provider] = (state.pools[stored.provider] ?? []).filter((ref) => ref.account_id !== accountId);
        return new Response(null, { status: 204 });
      }
      if (method === "GET") {
        if (state.signInLands.includes(stored.account_id)) Object.assign(stored, { logged_in: true, available: true, reason: "" });
        return json(200, stored);
      }
      if (typeof body.label === "string") stored.label = body.label;
      if (typeof body.enabled === "boolean") stored.enabled = body.enabled;
      if (typeof body.reserve_percent === "number") stored.reserve_percent = body.reserve_percent;
      return json(200, stored);
    }
    return undefined;
  };
}

export function modelsDaemon(patch: Partial<ModelState> = {}) {
  const state = modelState(patch);
  const routes = modelRoutes(state);
  const fake = fakeClient((call) => routes(call) ?? json(404, { code: "not_found", message: "nope" }));
  return { ...fake, state };
}
