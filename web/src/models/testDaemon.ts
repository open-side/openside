import type { AccountsResponse, ModelAccountRef, ModelCatalog, OAuthAccount, OAuthMethod, OAuthProviderInfo, OAuthTransaction, ProviderConnection, ProviderConnectionList, ProviderKeyInfo, SubscriptionAccount } from "../api/types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { TERMINAL_STATUS } from "./OAuthDialogs";

const NEEDS_CREDENTIAL = "No credential yet: add an API key or sign in to a subscription.";
const NOT_IN_POOL = "No account is in the rotation.";

interface Curated {
  id: string;
  label: string;
  display_name: string;
  kind: "chat" | "image";
  recommended_for: "fast" | "reasoning" | null;
  tier: "fast" | "reasoning" | "frontier";
  strengths: string;
  in_catalog: boolean;
}

const CURATED: Record<string, Curated[]> = {
  anthropic: [
    { id: "anthropic:claude-sonnet-5-5", label: "Claude Sonnet 5.5", display_name: "Claude Sonnet 5.5", kind: "chat", recommended_for: "reasoning", tier: "reasoning", strengths: "Multi-step research, coding", in_catalog: true },
    { id: "anthropic:claude-haiku-5-5", label: "Claude Haiku 5.5", display_name: "Claude Haiku 5.5", kind: "chat", recommended_for: "fast", tier: "fast", strengths: "Quick lookups, simple edits", in_catalog: true },
  ],
  openai: [
    { id: "openai:gpt-6.1-sol", label: "GPT-6.1 Sol", display_name: "GPT-6.1 Sol", kind: "chat", recommended_for: "reasoning", tier: "reasoning", strengths: "Multi-step research, coding", in_catalog: true },
    { id: "openai:gpt-6-astra", label: "GPT-6 Astra", display_name: "GPT-6 Astra", kind: "chat", recommended_for: "reasoning", tier: "frontier", strengths: "Hard reasoning, architecture", in_catalog: true },
    { id: "openai:gpt-6-luna", label: "GPT-6 Luna", display_name: "GPT-6 Luna", kind: "chat", recommended_for: "fast", tier: "fast", strengths: "Quick lookups, simple edits", in_catalog: true },
    { id: "openai:gpt-image-2", label: "GPT Image 2", display_name: "GPT Image 2", kind: "image", recommended_for: null, tier: "reasoning", strengths: "", in_catalog: true },
  ],
  google: [{ id: "google-gla:gemini-2.5-pro", label: "Gemini 2.5 Pro", display_name: "", kind: "chat", recommended_for: null, tier: "reasoning", strengths: "", in_catalog: false }],
};

const SUBSCRIPTION_PROVIDER: Record<string, string> = { "claude-code": "anthropic", "codex-cli": "openai" };

/** The contract's first-release allowlist, in its order. */
export const OAUTH_PROVIDERS: OAuthProviderInfo[] = [
  { provider: "anthropic", name: "Claude (Pro/Max)", methods: ["browser"], manual_input_methods: ["browser"] },
  { provider: "openai-codex", name: "ChatGPT (Codex)", methods: ["browser", "device"], manual_input_methods: ["browser"] },
  { provider: "google-gemini-cli", name: "Google Gemini CLI", methods: ["browser"], manual_input_methods: ["browser"] },
  { provider: "google-antigravity", name: "Google Antigravity", methods: ["browser"], manual_input_methods: ["browser"] },
  { provider: "github-copilot", name: "GitHub Copilot", methods: ["device"], manual_input_methods: [] },
];

/** A transaction plus what the fake needs to commit it. */
interface FakeTransaction {
  transaction: OAuthTransaction;
  label: string;
  joinPool: boolean;
}

export interface ModelState {
  /** False: a read-scope device (key and subscription lists and every write answer 403). */
  admin: boolean;
  keys: ProviderKeyInfo[];
  subscriptions: SubscriptionAccount[];
  /** Which subscription CLIs exist on the daemon's computer. */
  installed: Record<string, boolean>;
  pools: Record<string, ModelAccountRef[]>;
  defaultModel: string | null;
  /** Null: the fast role follows the session model. */
  fastModel: string | null;
  standardModel: string | null;
  deepModel: string | null;
  visualModel: string | null;
  /** Null: automatic (the first usable image model); "off": image generation off. */
  imageModel: string | null;
  fallbacks: string[];
  accounts: AccountsResponse;
  /** Every key value the daemon was sent, for "never rendered back" assertions. */
  received: string[];
  /** Answer key writes with 409 vault_unavailable. */
  vaultLocked: boolean;
  /** Accounts of other sources (broker, env, ...) per provider, as the catalog lists them. */
  extra: Record<string, Partial<Candidate>[]>;
  oauthAccounts: OAuthAccount[];
  oauthTransactions: Record<string, FakeTransaction>;
  /** What the next poll of a transaction reports: `ready` commits it, any other value fails it with that reason. */
  oauthOutcome: Record<string, string>;
  /** Holds the next poll of a transaction until the promise settles, answering with the state it had when asked. */
  oauthGate: Record<string, Promise<void>>;
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
    standardModel: null,
    deepModel: null,
    visualModel: null,
    imageModel: null,
    fallbacks: [],
    accounts: { sources: [], cached_for_ms: 0, truncated: false },
    received: [],
    vaultLocked: false,
    extra: {},
    oauthAccounts: [],
    oauthTransactions: {},
    oauthOutcome: {},
    oauthGate: {},
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

function oauthReadiness(account: OAuthAccount): OAuthAccount {
  const available = account.enabled && !account.auth_invalid;
  const reason = !account.enabled ? "Account disabled" : account.auth_invalid ? "Sign in again" : "OAuth account ready";
  return Object.assign(account, { available, reason });
}

export function oauthAccount(provider: string, accountId: string, label: string, patch: Partial<OAuthAccount> = {}): OAuthAccount {
  return oauthReadiness({
    source: "pi-oauth",
    provider,
    account_id: accountId,
    label,
    enabled: true,
    reserve_percent: 10,
    available: true,
    reason: "OAuth account ready",
    auth_invalid: false,
    created_at_ms: 1_760_261_400_000,
    ...patch,
  });
}

/** Moves a started transaction to the step its method waits on, as the helper's `auth` event does. */
function authorize(fake: FakeTransaction) {
  const tx = fake.transaction;
  if (tx.status !== "starting") return;
  const url = `https://auth.example.test/authorize?tx=${tx.transaction_id}`;
  if (tx.method === "device") {
    Object.assign(tx, { status: "awaiting_code", authorization: { url, user_code: "ABCD-1234", instructions: "Enter code: ABCD-1234" } });
  } else {
    const manual = OAUTH_PROVIDERS.find((info) => info.provider === tx.provider)!.manual_input_methods.includes("browser");
    Object.assign(tx, { status: "awaiting_browser", authorization: { url, launch_url: "http://127.0.0.1:53692/start" }, manual_input: manual });
  }
}

function settle(state: ModelState, fake: FakeTransaction, outcome: string) {
  const tx = fake.transaction;
  Object.assign(tx, { authorization: null, manual_input: false });
  if (outcome !== "ready") {
    Object.assign(tx, { status: "failed", reason: outcome });
    return;
  }
  let account = state.oauthAccounts.find((candidate) => candidate.provider === tx.provider && candidate.account_id === tx.account_id);
  if (account) {
    oauthReadiness(Object.assign(account, { auth_invalid: false }));
  } else {
    account = oauthAccount(tx.provider, tx.account_id, fake.label);
    state.oauthAccounts.push(account);
    // Like the daemon: the pool changes only once the sign-in is committed.
    if (fake.joinPool) state.pools[tx.provider] = [...(state.pools[tx.provider] ?? []), { source: "pi-oauth", account_id: tx.account_id }];
  }
  Object.assign(tx, { status: "ready", reason: null, account: { ...account } });
}

function oauthRoutes(state: ModelState, key: string, body: Record<string, unknown>): Response | Promise<Response> | undefined {
  if (key === "GET /v1/settings/oauth/providers") return json(200, { providers: OAUTH_PROVIDERS });
  if (key === "GET /v1/settings/oauth/accounts") return json(200, { accounts: state.oauthAccounts });
  const start = (provider: string, accountId: string, kind: "create" | "relogin", method: OAuthMethod, label: string, joinPool: boolean) => {
    const id = `tx-${state.nextId++}`;
    const transaction: OAuthTransaction = {
      transaction_id: id,
      kind,
      provider,
      account_id: accountId,
      method,
      status: "starting",
      reason: null,
      authorization: null,
      manual_input: false,
      expires_at_ms: Date.now() + 600_000,
      account: null,
    };
    state.oauthTransactions[id] = { transaction, label, joinPool };
    return json(202, transaction);
  };

  const create = /^POST \/v1\/settings\/oauth\/accounts\/([^/]+)$/.exec(key);
  if (create) {
    const info = OAUTH_PROVIDERS.find((candidate) => candidate.provider === create[1]);
    if (!info) return json(400, { code: "oauth_provider_unknown", message: "unknown" });
    const method = (body.method as OAuthMethod | undefined) ?? info.methods[0]!;
    if (!info.methods.includes(method)) return json(400, { code: "oauth_flow_unsupported", message: "unsupported" });
    return start(info.provider, `oa-${state.nextId++}`, "create", method, String(body.label), body.join_pool === true);
  }

  const account = /^(GET|PATCH|DELETE|POST) \/v1\/settings\/oauth\/accounts\/([^/]+)\/([^/]+)(\/relogin)?$/.exec(key);
  if (account) {
    const [, method, provider, accountId, relogin] = account as unknown as [string, string, string, string, string | undefined];
    const stored = state.oauthAccounts.find((candidate) => candidate.provider === provider && candidate.account_id === accountId);
    if (!stored) return json(404, { code: "unknown_account", message: "none" });
    if (relogin && method === "POST") {
      const info = OAUTH_PROVIDERS.find((candidate) => candidate.provider === provider)!;
      return start(provider, accountId, "relogin", (body.method as OAuthMethod | undefined) ?? info.methods[0]!, stored.label, false);
    }
    if (method === "DELETE") {
      state.oauthAccounts = state.oauthAccounts.filter((candidate) => candidate !== stored);
      for (const [pool, refs] of Object.entries(state.pools)) state.pools[pool] = refs.filter((ref) => !(ref.source === "pi-oauth" && ref.account_id === accountId));
      return new Response(null, { status: 204 });
    }
    if (method === "PATCH") {
      if (typeof body.label === "string") stored.label = body.label;
      if (typeof body.enabled === "boolean") stored.enabled = body.enabled;
      if (typeof body.reserve_percent === "number") stored.reserve_percent = body.reserve_percent;
      oauthReadiness(stored);
    }
    return json(200, stored);
  }

  const transaction = /^(GET|POST) \/v1\/settings\/oauth\/transactions\/([^/]+)(?:\/(input|cancel))?$/.exec(key);
  if (transaction) {
    const [, method, id, action] = transaction as unknown as [string, string, string, string | undefined];
    const fake = state.oauthTransactions[id];
    if (!fake) return json(404, { code: "oauth_transaction_unknown", message: "unknown" });
    const tx = fake.transaction;
    if (method === "GET" && !action) {
      authorize(fake);
      const gate = state.oauthGate[id];
      if (gate) {
        delete state.oauthGate[id];
        const snapshot = structuredClone(tx);
        return gate.then(() => json(200, snapshot));
      }
      const outcome = state.oauthOutcome[id];
      if (outcome && !TERMINAL_STATUS[tx.status]) {
        delete state.oauthOutcome[id];
        settle(state, fake, outcome);
      }
      return json(200, tx);
    }
    if (action === "cancel") {
      if (!TERMINAL_STATUS[tx.status]) Object.assign(tx, { status: "cancelled", reason: "oauth_cancelled", authorization: null, manual_input: false });
      return new Response(null, { status: 204 });
    }
    if (action === "input") {
      if (TERMINAL_STATUS[tx.status]) return json(409, { code: "oauth_transaction_finished", message: "finished" });
      if (!tx.manual_input) return json(409, { code: "oauth_manual_input_not_allowed", message: "not now" });
      state.received.push(String(body.code));
      Object.assign(tx, { status: "exchanging", manual_input: false });
      state.oauthOutcome[id] ??= "ready";
      return json(202, tx);
    }
  }
  return undefined;
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
    ...state.oauthAccounts.filter((account) => account.provider === provider).map((account) => make("pi-oauth", account.account_id, account.label, account.enabled, !account.auth_invalid, account.reason)),
    ...(state.extra[provider] ?? []).map((extra) => ({ ...make(extra.source!, extra.account_id!, extra.label ?? extra.account_id!, true, true, ""), ...extra })),
  ];
}

function catalog(state: ModelState): ModelCatalog {
  const providers = Object.entries(CURATED).map(([provider, models]) => {
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
  });
  // Like the daemon: "off" resolves to nothing, null to the first usable image model.
  const effective =
    state.imageModel === "off"
      ? null
      : (state.imageModel ?? providers.flatMap((provider) => provider.models).find((model) => model.available && model.kind === "image")?.id ?? null);
  return {
    providers,
    default_model: state.defaultModel,
    fast_model: state.fastModel,
    standard_model: state.standardModel,
    deep_model: state.deepModel,
    visual_model: state.visualModel,
    image_model: state.imageModel,
    effective_image_model: effective,
    fallback_models: state.fallbacks,
    cached_for_ms: 0,
    truncated: false,
  } as unknown as ModelCatalog;
}

const forbidden = () => json(403, { code: "forbidden", message: "needs admin" });
const absent = () => json(404, { code: "key_not_found", message: "none" });

const PRODUCT: Record<string, string> = {
  anthropic: "Claude",
  "openai-codex": "ChatGPT",
  openai: "OpenAI",
  google: "Gemini",
  "google-gemini-cli": "Gemini CLI",
  "google-antigravity": "Antigravity",
  "github-copilot": "GitHub Copilot",
};

/** Like the daemon: one row per provider and connection kind, connected when any account is usable. */
function connections(state: ModelState): ProviderConnectionList {
  const rows = new Map<string, ProviderConnection>();
  const add = (provider: string, source: string, accountId: string, ready: boolean) => {
    const kind = ["vault", "env", "command", "builtin"].includes(source) ? "api_key" : "subscription";
    const id = `${provider}/${kind}`;
    const name = PRODUCT[provider] ?? provider;
    const row = rows.get(id) ?? {
      id,
      provider,
      kind,
      display_name: kind === "subscription" ? name : provider === "anthropic" ? "Anthropic API key" : `${name} API key`,
      status: "needs_attention",
      accounts: [],
    };
    if (ready) row.status = "connected";
    row.accounts.push({ source, account_id: accountId });
    rows.set(id, row);
  };
  for (const key of state.keys) add(key.provider, "vault", key.key_id, key.enabled);
  for (const sub of state.subscriptions) add(sub.provider, sub.source, sub.account_id, sub.enabled && sub.logged_in);
  for (const account of state.oauthAccounts) add(account.provider, "pi-oauth", account.account_id, account.enabled && !account.auth_invalid);
  for (const [provider, extras] of Object.entries(state.extra)) for (const extra of extras) add(provider, extra.source!, extra.account_id!, extra.ready ?? true);
  return { providers: [...rows.values()].sort((a, b) => (a.id < b.id ? -1 : 1)) };
}

/** The Models & accounts endpoints of the daemon; undefined for any other request. */
export function modelRoutes(state: ModelState): (call: Call) => Response | Promise<Response> | undefined {
  return (call) => {
    const path = new URL(call.path, "http://daemon.test").pathname;
    const key = `${call.method} ${path}`;
    const body = (call.body ?? {}) as Record<string, unknown>;
    if (key === "GET /v1/settings/models") return json(200, catalog(state));
    if (key === "GET /v1/settings/models/providers") return json(200, connections(state));
    if (key === "GET /v1/accounts") return json(200, state.accounts);
    if (key === "GET /v1/settings/pools") return json(200, { pools: Object.entries(state.pools).map(([provider, accounts]) => ({ provider, accounts })) });
    if (!state.admin) {
      const adminOnly = path === "/v1/settings/keys" || path.startsWith("/v1/settings/subscriptions") || path.startsWith("/v1/settings/oauth");
      if (call.method !== "GET" || adminOnly) return path.startsWith("/v1/settings") || call.method !== "GET" ? forbidden() : undefined;
    }
    if (path.startsWith("/v1/settings/oauth/")) return oauthRoutes(state, key, body);
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
      const request = call.body as { default_model: string; fast_model?: string | null; fallback_models: string[]; standard_model?: string | null; deep_model?: string | null; visual_model?: string | null; image_model?: string | null };
      const all = catalog(state).providers.flatMap((provider) => provider.models);
      const usable = (id: string | null | undefined, kind: string) => !id || all.some((model) => model.id === id && model.available && model.kind === kind);
      const chat = [request.default_model, request.fast_model, request.standard_model, request.deep_model, request.visual_model, ...request.fallback_models];
      const image = request.image_model === "off" ? null : request.image_model;
      const unusable = chat.find((id) => !usable(id, "chat")) ?? (usable(image, "image") ? undefined : image);
      if (unusable) return json(400, { code: "model_unavailable", message: `${unusable} is unavailable` });
      state.defaultModel = request.default_model;
      // Like the daemon: a fast model equal to the default is stored as null.
      state.fastModel = request.fast_model && request.fast_model !== request.default_model ? request.fast_model : null;
      // Omitted roles keep their value; an explicit null clears it.
      if (request.standard_model !== undefined) state.standardModel = request.standard_model;
      if (request.deep_model !== undefined) state.deepModel = request.deep_model;
      if (request.visual_model !== undefined) state.visualModel = request.visual_model;
      if (request.image_model !== undefined) state.imageModel = request.image_model;
      state.fallbacks = request.fallback_models;
      return json(200, {
        default_model: request.default_model,
        fast_model: state.fastModel,
        standard_model: state.standardModel,
        deep_model: state.deepModel,
        visual_model: state.visualModel,
        image_model: state.imageModel,
        fallback_models: request.fallback_models,
        restart_required: false,
        effective: "new_sessions_and_home_next_turn",
      });
    }

    const pool = /^PUT \/v1\/settings\/pools\/([^/]+)$/.exec(key);
    if (pool) {
      state.pools[pool[1]!] = body.accounts as ModelAccountRef[];
      return json(200, { provider: pool[1], accounts: state.pools[pool[1]!] });
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
      if (method === "GET") return json(200, stored);
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
