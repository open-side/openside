import type {
  AccountsResponse,
  ApiErrorBody,
  ArtifactList,
  BrowserTabList,
  CreateSessionRequest,
  Health,
  Message,
  Session,
  SessionList,
  Settings,
  SkillList,
  Subagent,
  SubmitMessageRequest,
  Ticket,
  TicketList,
  UpdateSessionRequest,
  VisualList,
  CreateMemoryRequest,
  CreatePushSubscriptionRequest,
  CreateRoutineRequest,
  CreateSkillRequest,
  DeviceList,
  ExternalAgentList,
  ImportProfileList,
  ImportReport,
  ImportRequest,
  InstalledAgentSkill,
  Memory,
  MemoryHealth,
  MemoryHistory,
  MemoryKind,
  MemorySearchResponse,
  OnboardingState,
  OnboardingStatus,
  PairDeviceResponse,
  PushSubscription,
  PushSubscriptionList,
  Routine,
  RoutineList,
  RoutineSuggestionList,
  RunList,
  RunRecord,
  SettingsDocument,
  SkillDetail,
  UpdateMemoryRequest,
  UpdateRoutineRequest,
  UpdateSkillRequest,
  VapidPublicKey,
  CreateVaultItemRequest,
  VaultAuditList,
  VaultItem,
  VaultItemList,
  VaultPolicyOverride,
  VaultStatus,
} from "./types";

/** Daemon error codes with a dedicated explanation; anything else shows the daemon message. */
const MESSAGES: Record<string, string> = {
  network_error: "Cannot reach the openside daemon. Check the connection and try again.",
  unauthorized: "The token was rejected. Paste a valid device token.",
  forbidden: "This token does not have permission for that action.",
  not_found: "Not found. It may have been deleted.",
  invalid_transition: "That action is no longer possible; the item already changed.",
  storage_full: "The computer running openside is out of disk space. Free some space and try again.",
  invalid_value: "The daemon rejected that value.",
  invalid_title: "Titles must be 1–200 characters.",
  empty_message: "Write a message or attach a file first.",
  model_not_configured: "No model is configured. Pick or type a model.",
  secret_input_unavailable: "Secret input is not available on this daemon right now.",
  origin_mismatch: "The page changed origin, so the secret was not filled.",
  needs_desktop: "This step must be completed on the desktop.",
  use_done: "Mark this step as done instead of answering it.",
  attachments_too_large: "The attachments are too large. Remove some and try again.",
  unsupported_attachment: "That file type cannot be attached.",
  takeover_in_use: "Another device is already controlling this step.",
  tab_unavailable: "The browser tab for this step is gone.",
  local_socket_required:
    "Only the computer running openside can make this change (local socket); paired devices cannot.",
  invalid_config: "The daemon rejected these settings: a value is out of range or conflicts with another key.",
  file_only_section: "That section can only be edited in config.toml.",
  config_file_invalid: "config.toml on the daemon is invalid; fix it on the computer running openside.",
  invalid_code: "This pairing link is malformed. Scan the QR code again.",
  too_many_attempts: "Too many failed pairing attempts. Wait a minute and try again.",
  pairing_expired: "This pairing code has expired. Create a new one with `openside pair`.",
  pairing_not_found: "This pairing code was already used or is unknown. Create a new one with `openside pair`.",
  secret_detected: "That text looks like a secret, so it was not stored.",
  unknown_subscription: "This device's notification subscription is no longer registered.",
  vault_unavailable: "The vault is locked or not set up. Unlock it first.",
  vault_already_set_up: "The vault is already set up.",
  vault_not_set_up: "The vault is not set up yet.",
  vault_unsupported: "This computer cannot keep a vault with the configured unlock method. Run `openside doctor` there.",
  keyring_unavailable: "The computer's keyring is locked or unreachable. Unlock it on the computer and try again.",
  passphrase_required: "Enter the vault passphrase.",
  passphrase_not_used: "This vault is unlocked by the computer's keyring, not a passphrase.",
  wrong_passphrase: "Wrong passphrase.",
  weak_passphrase: "Use a passphrase of at least 8 characters.",
  invalid_origin: "Each origin must look like https://example.com.",
  invalid_totp: "That is not a valid authenticator setup key or otpauth:// link.",
  empty_item: "Add a password or an authenticator.",
  invalid_item: "Item ids use lowercase letters, digits, ':', '_' or '-' (up to 64).",
  item_exists: "An item with that id already exists.",
  unknown_item: "That item no longer exists.",
  unknown_agent: "That agent is not in the list.",
  skills_dir_unknown: "openside does not know where this agent keeps its skills. Set it under [skills.agents] in config.toml.",
  exists: "Already installed.",
  unknown_profile: "That browser profile is no longer there. Reload the list.",
  import_in_progress: "Another import is running. Wait for it to finish.",
  profile_in_use:
    "The agent browser is running, so cookies cannot be written now. Import history and bookmarks only, or try again when no conversation is using the browser.",
  source_locked: "The browser is holding its data locked. Close it and try again.",
  source_unreadable: "openside cannot read this profile. Check that the browser's folder is readable.",
  source_corrupt: "A database in this profile is damaged, so it cannot be read.",
  source_unsupported: "This profile uses a format openside cannot read yet.",
  keyring_password_required:
    "These cookies are locked by the computer's keyring. Leave cookies out here, or run openside import with --keyring-password-cmd on the computer.",
  browser_unavailable: "No Chromium-compatible browser was found, so cookies cannot be written. History and bookmarks can still be imported.",
  import_failed: "The import failed. Nothing was changed; see the daemon log for details.",
};

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message?: string) {
    super(MESSAGES[code] ?? message ?? `Request failed (${status})`);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/** Convert a failed response into an [`ApiError`], tolerating non-JSON bodies. */
export async function toApiError(response: Response): Promise<ApiError> {
  let body: Partial<ApiErrorBody> | undefined;
  try {
    body = (await response.json()) as Partial<ApiErrorBody>;
  } catch {
    body = undefined;
  }
  const code = typeof body?.code === "string" ? body.code : `http_${response.status}`;
  const message = typeof body?.message === "string" ? body.message : undefined;
  return new ApiError(response.status, code, message);
}

export interface ClientOptions {
  /** Same-origin by default; the daemon serves the UI and the API together. */
  baseUrl?: string;
  token: () => string | null;
  fetch?: typeof fetch;
  /** Signed iframe navigation cannot use a relay's encrypted fetch replacement. */
  relay?: boolean;
  /** Called on 401 so the shell can ask for a new token. */
  onUnauthorized?: () => void;
  /** WebSocket factory; tests inject a fake. */
  webSocket?: (url: string, protocols: string[]) => WebSocket;
}

export interface ListSessionsQuery {
  q?: string;
  archived?: boolean;
}

export interface MemoryQuery {
  q?: string;
  kind?: MemoryKind;
  limit?: number;
}

export interface VaultAuditQuery {
  /** RFC 3339 lower bound. */
  since?: string;
  item?: string;
  session?: string;
  limit?: number;
}

/** Typed wrapper over the v1 HTTP API; every UI action goes through one of these calls. */
export interface Client {
  baseUrl: string;
  readonly relay: boolean;
  /** Request headers carrying the bearer token, shared with the SSE reader. */
  headers(extra?: Record<string, string>): Record<string, string>;
  fetch: typeof fetch;
  health(): Promise<Health>;
  accounts(): Promise<AccountsResponse>;
  settings(): Promise<Settings>;
  listSessions(query?: ListSessionsQuery): Promise<SessionList>;
  createSession(request: CreateSessionRequest): Promise<Session>;
  updateSession(id: string, request: UpdateSessionRequest): Promise<Session>;
  stopSession(id: string): Promise<Session>;
  /** Start the session's agent process, browser and a blank agent tab ahead of a message. */
  prewarmSession(id: string): Promise<void>;
  /** Launch the browser ahead of a new chat's first message. */
  prewarmBrowser(): Promise<void>;
  submitMessage(id: string, request: SubmitMessageRequest): Promise<Message>;
  editQueued(id: string, messageId: string, text: string): Promise<Message>;
  deleteQueued(id: string, messageId: string): Promise<Message>;
  promoteQueued(id: string, messageId: string): Promise<Message>;
  /** The extension hydrates pending approval cards when its side panel reopens. */
  listTickets(status?: Ticket["status"]): Promise<TicketList>;
  resolveTicket(id: string, value: unknown): Promise<Ticket>;
  rejectTicket(id: string, reason?: string): Promise<Ticket>;
  ticketDone(id: string): Promise<Ticket>;
  cancelSubagent(id: string, subagentId: string): Promise<Subagent>;
  steerSubagent(id: string, subagentId: string, text: string): Promise<Subagent>;
  /** `/` mention candidates: enabled skills visible to the session. */
  searchSkills(sessionId: string, q: string): Promise<SkillList>;
  listArtifacts(id: string): Promise<ArtifactList>;
  listVisuals(id: string): Promise<VisualList>;
  /** Raw artifact bytes through the authenticated download route. */
  artifactBlob(id: string, path: string): Promise<Blob>;
  /** Fetch a signed preview through the selected transport without losing its network restrictions. */
  sandboxPreview(path: string, signal?: AbortSignal): Promise<Blob>;
  /** Absolute URL for a daemon-relative path such as a signed `preview_url`. */
  resolve(path: string): string;
  /**
   * Takeover stream for a ticket. Browsers cannot set Authorization on a WebSocket, so the
   * token travels as the `openside.bearer.<token>` subprotocol; the daemon echoes `openside.v1`.
   */
  takeover(ticketId: string): WebSocket;
  browserPreview(sessionId: string): WebSocket;
  browserThumbnail(sessionId: string, thumbnailId: string, signal?: AbortSignal): Promise<Blob>;
  /** Agent-owned, non-private tabs with sanitized URLs; 503 `browser_disconnected` when the controller is gone. */
  browserTabs(): Promise<BrowserTabList>;
  listSkills(): Promise<SkillList>;
  getSkill(name: string): Promise<SkillDetail>;
  createSkill(request: CreateSkillRequest): Promise<SkillDetail>;
  updateSkill(name: string, request: UpdateSkillRequest): Promise<SkillDetail>;
  deleteSkill(name: string): Promise<SkillDetail>;
  listRoutines(): Promise<RoutineList>;
  createRoutine(request: CreateRoutineRequest): Promise<Routine>;
  updateRoutine(id: string, request: UpdateRoutineRequest): Promise<Routine>;
  deleteRoutine(id: string): Promise<Routine>;
  runRoutine(id: string): Promise<RunRecord>;
  routineRuns(id: string): Promise<RunList>;
  routineSuggestions(): Promise<RoutineSuggestionList>;
  searchMemory(query?: MemoryQuery): Promise<MemorySearchResponse>;
  memoryHealth(): Promise<MemoryHealth>;
  createMemory(request: CreateMemoryRequest): Promise<Memory>;
  updateMemory(id: string, request: UpdateMemoryRequest): Promise<Memory>;
  deleteMemory(id: string): Promise<Memory>;
  memoryHistory(id: string): Promise<MemoryHistory>;
  settingsConfig(): Promise<SettingsDocument>;
  updateSettingsConfig(config: unknown): Promise<SettingsDocument>;
  vaultStatus(): Promise<VaultStatus>;
  /** The passphrase is only for a passphrase vault; a keyring vault takes none. */
  setupVault(passphrase?: string): Promise<VaultStatus>;
  unlockVault(passphrase?: string): Promise<VaultStatus>;
  lockVault(): Promise<VaultStatus>;
  createVaultItem(request: CreateVaultItemRequest): Promise<VaultItem>;
  deleteVaultItem(id: string): Promise<void>;
  vaultItems(): Promise<VaultItemList>;
  updateVaultItem(id: string, policy: VaultPolicyOverride): Promise<VaultItem>;
  vaultAudit(query?: VaultAuditQuery): Promise<VaultAuditList>;
  /** First-run guide state, kept on the daemon so every device agrees. */
  onboarding(): Promise<OnboardingStatus>;
  /** Record how the guide ended; reopening it from Settings does not touch the flag. */
  setOnboarding(state: Exclude<OnboardingState, "pending">): Promise<OnboardingStatus>;
  externalAgents(): Promise<ExternalAgentList>;
  /** Install the bundled browser skill into the agent's skills directory; 409 `exists` unless `force`. */
  installAgentSkill(agent: string, force?: boolean): Promise<InstalledAgentSkill>;
  importProfiles(): Promise<ImportProfileList>;
  /** `dry_run` reads and decrypts the profile and reports counts without writing. */
  runImport(request: ImportRequest): Promise<ImportReport>;
  listDevices(): Promise<DeviceList>;
  revokeDevice(id: string): Promise<void>;
  /** Public route: the one-time code is the credential, so no bearer token is needed. */
  exchangePairing(code: string): Promise<PairDeviceResponse>;
  vapidKey(): Promise<VapidPublicKey>;
  listPushSubscriptions(): Promise<PushSubscriptionList>;
  createPushSubscription(request: CreatePushSubscriptionRequest): Promise<PushSubscription>;
  deletePushSubscription(id: string): Promise<void>;
  pushPresence(subscriptionId: string, focused: boolean): Promise<void>;
}

export function createClient(options: ClientOptions): Client {
  const base = options.baseUrl ?? "";
  const rawFetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  // Every request, including the raw SSE stream, reports a rejected token once here.
  const doFetch: typeof fetch = async (input, init) => {
    const response = await rawFetch(input, init);
    if (response.status === 401) options.onUnauthorized?.();
    return response;
  };

  function headers(extra?: Record<string, string>): Record<string, string> {
    const result: Record<string, string> = { Accept: "application/json", ...extra };
    const token = options.token();
    if (token) result.Authorization = `Bearer ${token}`;
    return result;
  }

  async function send(path: string, init: RequestInit = {}): Promise<Response> {
    let response: Response;
    try {
      response = await doFetch(base + path, init);
    } catch (error) {
      if (init.signal?.aborted) throw error;
      throw new ApiError(0, "network_error");
    }
    if (!response.ok) throw await toApiError(response);
    return response;
  }

  async function json<T>(method: string, path: string, body?: unknown): Promise<T> {
    const init: RequestInit = { method, headers: headers() };
    if (body !== undefined) {
      init.headers = headers({ "Content-Type": "application/json" });
      init.body = JSON.stringify(body);
    }
    const response = await send(path, init);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  const enc = encodeURIComponent;

  function resolve(path: string): string {
    return new URL(base + path, globalThis.location?.href ?? "http://localhost/").toString();
  }

  function openWebSocket(path: string): WebSocket {
    const url = resolve(path).replace(/^http/, "ws");
    const protocols = ["openside.v1"];
    const token = options.token();
    if (token) protocols.push(`openside.bearer.${token}`);
    return options.webSocket ? options.webSocket(url, protocols) : new WebSocket(url, protocols);
  }

  return {
    baseUrl: base,
    relay: options.relay === true,
    headers,
    fetch: doFetch,
    health: () => json<Health>("GET", "/v1/health"),
    accounts: () => json<AccountsResponse>("GET", "/v1/accounts"),
    settings: () => json<Settings>("GET", "/v1/settings"),
    listSessions: (query: ListSessionsQuery = {}) => {
      const params = new URLSearchParams();
      if (query.q?.trim()) params.set("q", query.q.trim());
      if (query.archived) params.set("archived", "true");
      const suffix = params.size ? `?${params}` : "";
      return json<SessionList>("GET", `/v1/sessions${suffix}`);
    },
    createSession: (request: CreateSessionRequest) => json<Session>("POST", "/v1/sessions", request),
    updateSession: (id: string, request: UpdateSessionRequest) =>
      json<Session>("PATCH", `/v1/sessions/${enc(id)}`, request),
    stopSession: (id: string) => json<Session>("POST", `/v1/sessions/${enc(id)}/stop`),
    prewarmSession: async (id: string) => {
      await send(`/v1/sessions/${enc(id)}/prewarm`, { method: "POST", headers: headers() });
    },
    prewarmBrowser: async () => {
      await send("/v1/browser/prewarm", { method: "POST", headers: headers() });
    },
    submitMessage: (id: string, request: SubmitMessageRequest) =>
      json<Message>("POST", `/v1/sessions/${enc(id)}/messages`, request),
    editQueued: (id: string, messageId: string, text: string) =>
      json<Message>("PATCH", `/v1/sessions/${enc(id)}/queue/${enc(messageId)}`, { text }),
    deleteQueued: (id: string, messageId: string) =>
      json<Message>("DELETE", `/v1/sessions/${enc(id)}/queue/${enc(messageId)}`),
    promoteQueued: (id: string, messageId: string) =>
      json<Message>("POST", `/v1/sessions/${enc(id)}/queue/${enc(messageId)}/promote`),
    listTickets: (status?: Ticket["status"]) =>
      json<TicketList>("GET", `/v1/tickets${status ? `?status=${enc(status)}` : ""}`),
    resolveTicket: (id: string, value: unknown) =>
      json<Ticket>("POST", `/v1/tickets/${enc(id)}/resolve`, { value }),
    rejectTicket: (id: string, reason?: string) =>
      json<Ticket>("POST", `/v1/tickets/${enc(id)}/reject`, { reason: reason ?? null }),
    ticketDone: (id: string) => json<Ticket>("POST", `/v1/tickets/${enc(id)}/done`),
    cancelSubagent: (id: string, subagentId: string) =>
      json<Subagent>("POST", `/v1/sessions/${enc(id)}/subagents/${enc(subagentId)}/cancel`),
    steerSubagent: (id: string, subagentId: string, text: string) =>
      json<Subagent>("POST", `/v1/sessions/${enc(id)}/subagents/${enc(subagentId)}/steer`, { text }),
    searchSkills: (sessionId: string, q: string) => {
      const params = new URLSearchParams({ q, session: sessionId, enabled: "true" });
      return json<SkillList>("GET", `/v1/skills?${params}`);
    },
    listArtifacts: (id: string) => json<ArtifactList>("GET", `/v1/sessions/${enc(id)}/artifacts`),
    listVisuals: (id: string) => json<VisualList>("GET", `/v1/sessions/${enc(id)}/visuals`),
    artifactBlob: async (id: string, path: string) => {
      // The artifact path is one percent-encoded segment (`/` as %2F).
      const response = await send(`/v1/sessions/${enc(id)}/artifacts/${enc(path)}`, { headers: headers() });
      return response.blob();
    },
    sandboxPreview: async (path: string, signal?: AbortSignal) => {
      const response = await send(path, { signal });
      const blob = await response.blob();
      const mime = blob.type.split(";")[0];
      if (mime !== "text/html" && mime !== "image/svg+xml" && !mime?.startsWith("text/") && mime !== "application/json") {
        return blob;
      }
      // Blob navigation cannot carry HTTP security headers. Put the daemon's restrictions
      // before attacker-controlled markup; the empty iframe sandbox supplies the opaque origin.
      const csp = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; form-action 'none'; base-uri 'none'";
      const text = await blob.text();
      const content = mime === "text/html" || mime === "image/svg+xml"
        ? text
        : `<pre>${text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")}</pre>`;
      return new Blob([`<!doctype html><meta http-equiv="Content-Security-Policy" content="${csp}">`, content], { type: "text/html" });
    },
    resolve,
    takeover: (ticketId: string) => openWebSocket(`/v1/tickets/${enc(ticketId)}/takeover`),
    browserPreview: (sessionId: string) => openWebSocket(`/v1/sessions/${enc(sessionId)}/browser/preview`),
    browserThumbnail: async (sessionId: string, thumbnailId: string, signal?: AbortSignal) => {
      const response = await send(`/v1/sessions/${enc(sessionId)}/browser/thumbnails/${enc(thumbnailId)}`, {
        headers: headers({ Accept: "image/jpeg" }), signal,
      });
      return response.blob();
    },
    browserTabs: () => json<BrowserTabList>("GET", "/v1/browser/tabs"),
    listSkills: () => json<SkillList>("GET", "/v1/skills"),
    getSkill: (name: string) => json<SkillDetail>("GET", `/v1/skills/${enc(name)}`),
    createSkill: (request: CreateSkillRequest) => json<SkillDetail>("POST", "/v1/skills", request),
    updateSkill: (name: string, request: UpdateSkillRequest) =>
      json<SkillDetail>("PATCH", `/v1/skills/${enc(name)}`, request),
    deleteSkill: (name: string) => json<SkillDetail>("DELETE", `/v1/skills/${enc(name)}`),
    listRoutines: () => json<RoutineList>("GET", "/v1/routines"),
    createRoutine: (request: CreateRoutineRequest) => json<Routine>("POST", "/v1/routines", request),
    updateRoutine: (id: string, request: UpdateRoutineRequest) =>
      json<Routine>("PATCH", `/v1/routines/${enc(id)}`, request),
    deleteRoutine: (id: string) => json<Routine>("DELETE", `/v1/routines/${enc(id)}`),
    runRoutine: (id: string) => json<RunRecord>("POST", `/v1/routines/${enc(id)}/run`),
    routineRuns: (id: string) => json<RunList>("GET", `/v1/routines/${enc(id)}/runs`),
    routineSuggestions: () => json<RoutineSuggestionList>("GET", "/v1/routines/suggestions"),
    searchMemory: (query: MemoryQuery = {}) => {
      const params = new URLSearchParams();
      if (query.q?.trim()) params.set("q", query.q.trim());
      if (query.kind) params.set("kind", query.kind);
      if (query.limit) params.set("limit", String(query.limit));
      const suffix = params.size ? `?${params}` : "";
      return json<MemorySearchResponse>("GET", `/v1/memory${suffix}`);
    },
    memoryHealth: () => json<MemoryHealth>("GET", "/v1/memory/health"),
    createMemory: (request: CreateMemoryRequest) => json<Memory>("POST", "/v1/memory", request),
    updateMemory: (id: string, request: UpdateMemoryRequest) =>
      json<Memory>("PATCH", `/v1/memory/${enc(id)}`, request),
    deleteMemory: (id: string) => json<Memory>("DELETE", `/v1/memory/${enc(id)}`),
    memoryHistory: (id: string) => json<MemoryHistory>("GET", `/v1/memory/${enc(id)}/history`),
    settingsConfig: () => json<SettingsDocument>("GET", "/v1/settings/config"),
    updateSettingsConfig: (config: unknown) =>
      json<SettingsDocument>("PUT", "/v1/settings/config", { config }),
    vaultStatus: () => json<VaultStatus>("GET", "/v1/vault"),
    setupVault: (passphrase?: string) => json<VaultStatus>("POST", "/v1/vault/setup", passphrase ? { passphrase } : {}),
    unlockVault: (passphrase?: string) => json<VaultStatus>("POST", "/v1/vault/unlock", passphrase ? { passphrase } : {}),
    lockVault: () => json<VaultStatus>("POST", "/v1/vault/lock"),
    createVaultItem: (request: CreateVaultItemRequest) => json<VaultItem>("POST", "/v1/vault/items", request),
    deleteVaultItem: (id: string) => json<void>("DELETE", `/v1/vault/items/${enc(id)}`),
    vaultItems: () => json<VaultItemList>("GET", "/v1/vault/items"),
    updateVaultItem: (id: string, policy: VaultPolicyOverride) =>
      json<VaultItem>("PATCH", `/v1/vault/items/${enc(id)}`, { policy }),
    vaultAudit: (query: VaultAuditQuery = {}) => {
      const params = new URLSearchParams();
      if (query.since) params.set("since", query.since);
      if (query.item?.trim()) params.set("item", query.item.trim());
      if (query.session?.trim()) params.set("session", query.session.trim());
      if (query.limit) params.set("limit", String(query.limit));
      const suffix = params.size ? `?${params}` : "";
      return json<VaultAuditList>("GET", `/v1/vault/audit${suffix}`);
    },
    onboarding: () => json<OnboardingStatus>("GET", "/v1/onboarding"),
    setOnboarding: (state: Exclude<OnboardingState, "pending">) =>
      json<OnboardingStatus>("PUT", "/v1/onboarding", { state }),
    externalAgents: () => json<ExternalAgentList>("GET", "/v1/external-agents"),
    installAgentSkill: (agent: string, force = false) =>
      json<InstalledAgentSkill>("POST", `/v1/external-agents/${enc(agent)}/browser-skill`, { force }),
    importProfiles: () => json<ImportProfileList>("GET", "/v1/import/profiles"),
    runImport: (request: ImportRequest) => json<ImportReport>("POST", "/v1/import", request),
    listDevices: () => json<DeviceList>("GET", "/v1/devices"),
    revokeDevice: (id: string) => json<void>("DELETE", `/v1/devices/${enc(id)}`),
    exchangePairing: (code: string) =>
      json<PairDeviceResponse>("POST", "/v1/pairing/exchange", { code }),
    vapidKey: () => json<VapidPublicKey>("GET", "/v1/push/vapid"),
    listPushSubscriptions: () => json<PushSubscriptionList>("GET", "/v1/push/subscriptions"),
    createPushSubscription: (request: CreatePushSubscriptionRequest) =>
      json<PushSubscription>("POST", "/v1/push/subscriptions", request),
    deletePushSubscription: (id: string) => json<void>("DELETE", `/v1/push/subscriptions/${enc(id)}`),
    pushPresence: (subscriptionId: string, focused: boolean) =>
      json<void>("POST", "/v1/push/presence", { subscription_id: subscriptionId, focused }),
  };
}
