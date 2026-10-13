import type {
  AccountsResponse,
  ApiErrorBody,
  ArtifactList,
  BrowserTabList,
  CreateSessionRequest,
  Health,
  Message,
  ModelAccountRef,
  ModelCatalog,
  ModelsSaved,
  ModelsUpdate,
  ProviderKeyInfo,
  ProviderKeyList,
  ProviderPool,
  ProviderPoolList,
  Session,
  SessionList,
  Settings,
  SkillList,
  Subagent,
  SessionAccountSelection,
  SubscriptionAccount,
  SubscriptionList,
  SubmitMessageRequest,
  Ticket,
  TicketList,
  UpdateSessionRequest,
  VisualList,
  CreateMemoryRequest,
  CreatePushSubscriptionRequest,
  CreateRoutineRequest,
  CreateSkillRequest,
  CreatePairingRequest,
  AccessRequest,
  AccessRequestList,
  CapabilityScope,
  Device,
  MyDevice,
  DeviceList,
  ExternalAgentList,
  ImportProfileList,
  ImportBatchReport,
  ImportJob,
  ImportCategoryReport,
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
  PairingOffer,
  RemoteStatus,
  SetCloudflareRequest,
  SetRelayRequest,
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
  DaemonStatus,
  DoctorReport,
  MemoryModelStatus,
  NetworkPolicy,
  PairSyncRequest,
  PinnedTab,
  PinnedTabList,
  PolicyDecision,
  PolicySettings,
  PolicyTestRequest,
  PolicyValidation,
  RestartDaemonResponse,
  StartMemoryModelRequest,
  SyncReport,
  SyncSecrets,
  SyncSetupResponse,
  SyncStatus,
  UpdateSyncConfigRequest,
  ValidatePolicyRequest,
  ExternalAgent,
  Agent,
  AgentList,
  HireDraft,
  UpdateAgentRequest,
  RetireAgentRequest,
  ChannelAccountList,
  ChannelAccount,
  ChannelAccountTest,
  AddChannelAccountRequest,
  UpdateChannelAccountRequest,
  SuppressedProposalList,
  Connection,
  ConnectionAuditAction,
  ConnectionAuditList,
  ConnectionList,
  ConnectionStatus,
  ReloginResult,
  CreateAccountSpaceResult,
  RevokeConnectionRequest,
  UpdateConnectionRequest,
} from "./types";
import type { ImportDeliveryList } from "../import/protocol";

/** Daemon error codes with a dedicated explanation; anything else shows the daemon message. */
export const MESSAGES: Record<string, string> = {
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
  local_access_required: "Reopen openside using the desktop launcher on the computer it runs on. Other devices can view these settings but cannot change them.",
  scope_ceiling_exceeded: "This browser cannot grant a phone more access than it has itself. Remove a permission and try again.",
  person_approval_required: "Only people’s devices can be connected here.",
  transport_unavailable: "That connection cannot start yet. Check the hint on its card and try again.",
  transport_not_enabled: "Turn that connection on before connecting a phone through it.",
  invalid_config: "The daemon rejected these settings: a value is out of range or conflicts with another key.",
  file_only_section: "That section can only be edited in config.toml.",
  config_file_invalid: "config.toml on the daemon is invalid; fix it on the computer running openside.",
  invalid_code: "This pairing link is malformed. Scan the QR code again.",
  too_many_attempts: "Too many failed pairing attempts. Wait a minute and try again.",
  pairing_expired: "This pairing code has expired. Make a new one in openside on your computer (Settings → Remote access).",
  pairing_not_found: "This pairing code was already used or is unknown. Make a new one in openside on your computer (Settings → Remote access).",
  secret_detected: "That text looks like a secret, so it was not stored.",
  unknown_subscription: "This device's notification subscription is no longer registered.",
  vault_unavailable: "The vault is locked or not set up. Unlock it first.",
  invalid_models: "Pick a reasoning model, an optional fast model and up to 16 different fallback models.",
  model_unavailable: "That model is not available right now. Connect its provider first.",
  key_id_conflict: "A vault item already uses the name this key is stored under. Rename that item in the Vault, then try again.",
  invalid_provider: "Keys can be stored for Anthropic, OpenAI and Google.",
  key_not_found: "That key no longer exists. Reload the list.",
  invalid_source: "That subscription is not one openside can use.",
  person_device_required: "Only the computer running openside or a device you paired as yourself can make this change.",
  vault_already_set_up: "The vault is already set up.",
  vault_not_set_up: "The vault is not set up yet.",
  vault_unsupported: "This computer cannot keep a vault with the configured unlock method. Choose another unlock method under Configuration → vault.",
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
  skills_dir_unknown: "openside does not know where this agent keeps its skills. Add its skills folder in Agents.",
  invalid_agent: "Agent names use lowercase letters, digits and hyphens, and cannot reuse the name of a built-in agent.",
  invalid_skills_dir: "Use the full path of a folder inside your home folder, or one starting with ~/. It cannot contain “..”.",
  exists: "Already installed.",
  unknown_profile: "That browser profile is no longer there. Reload the list.",
  import_in_progress: "Another import is running. Wait for it to finish.",
  browser_unavailable: "No Chromium-compatible browser was found, so cookies cannot be written. History and bookmarks can still be imported.",
  vault_setup_required: "Set up the openside Vault before importing saved passwords.",
  vault_locked: "Unlock the openside Vault before importing saved passwords.",
  invalid_password_csv: "Choose a browser-exported CSV file with URL, username and password columns.",
  unknown_import_job: "That import is no longer available. Start a new import to finish.",
  import_write_failed: "The import did not finish. Data already imported is kept. Try again to finish.",
  import_interrupted: "The import stopped when openside restarted. Data already imported is kept. Import again to finish.",
  import_failed: "The import did not finish. Data already imported is kept. Check the summary, then try again.",
  invalid_model_source: "That model source is not valid. Use a repository like owner/name and an https address.",
  download_in_progress: "A model download is already running.",
  checksum_mismatch: "The downloaded model did not match its checksum, so it was discarded. Try again.",
  checksum_unavailable: "The model source did not publish checksums, so openside would not trust the download.",
  egress_refused: "The network allow-list does not include the model source. Add it under Policy → Network, restart openside, and try again.",
  download_failed: "The download did not finish. Check the connection and try again.",
  unsupported_model: "That repository does not hold a model openside can load.",
  invalid_policy: "The policy has an entry openside cannot use. Check the marked rows.",
  invalid_network_allow: "Each address must look like host, *.domain, host:port or https://host.",
  invalid_sync_config: "Choose a folder or a WebDAV address (https, or http on this computer), not both.",
  sync_already_set_up: "This device already joined a sync account, so its storage location cannot change.",
  sync_not_configured: "Choose where to sync first.",
  invalid_sync_secret: "That passphrase or password is empty or has characters openside cannot use. Type it again.",
  sync_not_set_up: "Start a sync account or join one first.",
  sync_wrong_passphrase: "Wrong sync passphrase.",
  sync_passphrase_unavailable: "Type the sync passphrase to unlock sync.",
  sync_password_unavailable: "Type the WebDAV password to unlock sync.",
  sync_backend_unavailable: "The sync storage cannot be reached right now.",
  sync_unauthorized: "The sync storage rejected the username or password.",
  sync_busy: "A sync is already running.",
  sync_counter_exhausted: "This sync account cannot take more changes. Start a new account on this device.",
  invalid_pairing_code: "That sync code is not valid. Copy it again from the first device.",
  invalid_url: "Use a full web address starting with http:// or https://.",
  agent_not_found: "That teammate no longer exists. Reload the list.",
  agent_retired: "That teammate is retired. Restore them first.",
  name_taken: "Someone on your team already has that name. Pick another.",
  roster_full: "Your team is full. Give the work to someone already on it, or retire a teammate first.",
  responsibility_taken: "A teammate already owns that responsibility. Give the work to them instead.",
  already_assigned: "Someone is already working on that.",
  channel_binding_conflict: "That channel already belongs to another teammate. Unassign it there first.",
  invalid_channel_kind: "That service is not supported. Choose Telegram, Slack or Discord.",
  invalid_channel_account: "Check the account: use a short id of lowercase letters, digits, - or _, a name, and at least one allowed chat, channel or user before turning it on.",
  channel_account_not_found: "That account no longer exists. Reload the list.",
  channel_account_exists: "An account with that id already exists. Pick another id.",
  channel_account_limit: "openside cannot connect more accounts. Remove one first.",
  channel_secret_conflict: "A vault item already uses the name this account's credentials are stored under. Rename that item in the Vault, then try again.",
  channel_ok: "The service accepted this account's credentials.",
  channel_credentials_expired: "The service refused these credentials. Replace them to reconnect.",
  channel_unavailable: "openside could not reach the service. Check the connection and test again.",
  chief_immutable: "Your assistant can be renamed, but not retired or replaced.",
  invalid_persona: "Check the profile: name up to 24 characters, a specific role up to 60, at least one responsibility for a teammate.",
  invalid_packet: "A handoff needs a goal and what done looks like.",
  invalid_account_pool: "Pick at least one account, or use the default accounts.",
  not_requested: "Only you can ask for a new teammate.",
  reviewer_is_author: "The reviewer must be someone other than the person who did the work.",
  connection_not_found: "That sign-in is no longer recorded. Reload the list.",
  connection_denied: "That agent is not allowed to use this sign-in.",
  connection_needs_login: "The site asked to sign in again before it could be used.",
  connection_cross_site: "That address belongs to a different site than this sign-in.",
  connection_browser_unavailable: "The agent browser is not running, so openside cannot check or sign in to sites right now.",
  connection_write_needs_approval: "That change on the site needs your approval first.",
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
  /**
   * Per-boot proof that this browser was opened from the computer running the daemon. Sent only
   * on the remote-access calls that need it, never through a relay.
   */
  localSecret?: () => string | null;
  /** WebSocket factory; tests inject a fake. */
  webSocket?: (url: string, protocols: string[]) => WebSocket;
}

export interface ListSessionsQuery {
  q?: string;
  archived?: boolean;
  /** `none` lists conversations from before the team (History); an agent id lists that agent's threads. */
  agent?: string;
}

export interface MemoryQuery {
  q?: string;
  kind?: MemoryKind;
  limit?: number;
}

export interface ConnectionsQuery {
  /** Hide connections explicitly denied to this agent. */
  agent?: string;
  status?: ConnectionStatus;
}

export interface ConnectionAuditQuery {
  connection?: string;
  agent?: string;
  action?: ConnectionAuditAction;
  limit?: number;
}

export interface VaultAuditQuery {
  /** RFC 3339 lower bound. */
  since?: string;
  item?: string;
  session?: string;
  limit?: number;
}

/** Fields a person can change on one key or subscription account. */
export interface AccountPatch {
  label?: string;
  enabled?: boolean;
  /** 0–99: share of a usage window kept unused before openside rotates away. */
  reserve_percent?: number;
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
  /** Every model by provider with availability, plus the default, fallbacks and subscription logins. */
  modelCatalog(): Promise<ModelCatalog>;
  setModels(request: ModelsUpdate): Promise<ModelsSaved>;
  /** Metadata only (label, last four characters, state); key values never come back. */
  listApiKeys(): Promise<ProviderKeyList>;
  addApiKey(provider: string, request: { label: string; key: string }): Promise<ProviderKeyInfo>;
  replaceApiKey(provider: string, keyId: string, request: { key: string; label?: string }): Promise<ProviderKeyInfo>;
  updateApiKey(provider: string, keyId: string, patch: AccountPatch): Promise<ProviderKeyInfo>;
  deleteApiKey(provider: string, keyId: string): Promise<void>;
  listSubscriptions(): Promise<SubscriptionList>;
  addSubscriptionAccount(source: string, label: string): Promise<SubscriptionAccount>;
  /** Polled while the person signs in from a terminal. */
  subscriptionAccount(source: string, accountId: string): Promise<SubscriptionAccount>;
  updateSubscriptionAccount(source: string, accountId: string, patch: AccountPatch): Promise<SubscriptionAccount>;
  deleteSubscriptionAccount(source: string, accountId: string): Promise<void>;
  listPools(): Promise<ProviderPoolList>;
  setPool(provider: string, accounts: ModelAccountRef[]): Promise<ProviderPool>;
  /** Preferred account for a conversation's later requests; `null` returns to automatic rotation. */
  setSessionAccount(id: string, account: ModelAccountRef | null): Promise<SessionAccountSelection>;
  listSessions(query?: ListSessionsQuery): Promise<SessionList>;
  /** The team: the chief and every teammate, with live thread state for the sidebar. */
  listAgents(): Promise<AgentList>;
  /** Admin from a person's device: hires directly, without a proposal card. */
  createAgent(draft: HireDraft): Promise<Agent>;
  /** Omitted fields stay; `null` on skills, model, account_pool or channel returns to the default. */
  updateAgent(id: string, patch: UpdateAgentRequest): Promise<Agent>;
  retireAgent(id: string, request: RetireAgentRequest): Promise<Agent>;
  restoreAgent(id: string): Promise<Agent>;
  /** Hire suggestions the person turned off with Never. */
  suppressedProposals(): Promise<SuppressedProposalList>;
  /** Lets the assistant suggest that teammate again. */
  clearSuppressedProposal(key: string): Promise<void>;
  /** Messaging identities (Telegram, Slack, Discord) a teammate can answer on; no credentials. */
  channelAccounts(): Promise<ChannelAccountList>;
  /** Admin from a person's device or the local socket. Credentials are write-only and never come back. */
  addChannelAccount(kind: string, request: AddChannelAccountRequest): Promise<ChannelAccount>;
  /** Omitted fields stay; `options` replaces every option; an empty `agent_id` clears the assignment. */
  updateChannelAccount(kind: string, accountId: string, patch: Partial<UpdateChannelAccountRequest>): Promise<ChannelAccount>;
  deleteChannelAccount(kind: string, accountId: string): Promise<void>;
  /** A read-only identity check with the service; never posts a message. */
  testChannelAccount(kind: string, accountId: string): Promise<ChannelAccountTest>;
  createSession(request: CreateSessionRequest): Promise<Session>;
  updateSession(id: string, request: UpdateSessionRequest): Promise<Session>;
  deleteSession(id: string): Promise<void>;
  stopSession(id: string): Promise<Session>;
  /** Start the session's agent process, browser and a blank agent tab ahead of a message. */
  prewarmSession(id: string): Promise<void>;
  submitMessage(id: string, request: SubmitMessageRequest): Promise<Message>;
  editQueued(id: string, messageId: string, text: string): Promise<Message>;
  deleteQueued(id: string, messageId: string): Promise<Message>;
  promoteQueued(id: string, messageId: string): Promise<Message>;
  /** The extension hydrates pending approval cards when its side panel reopens. */
  listTickets(status?: Ticket["status"]): Promise<TicketList>;
  /** `extra` carries sibling body fields of `value`, e.g. the edited `hire` draft of a hire ticket. */
  resolveTicket(id: string, value: unknown, extra?: Record<string, unknown>): Promise<Ticket>;
  rejectTicket(id: string, reason?: string): Promise<Ticket>;
  ticketDone(id: string): Promise<Ticket>;
  cancelSubagent(id: string, subagentId: string): Promise<Subagent>;
  /** `/` mention candidates: enabled skills visible to the session. */
  searchSkills(sessionId: string, q: string): Promise<SkillList>;
  listArtifacts(id: string): Promise<ArtifactList>;
  listVisuals(id: string): Promise<VisualList>;
  /** Raw artifact bytes through the authenticated download route. */
  artifactBlob(id: string, path: string): Promise<Blob>;
  /** The daemon's normalized PNG for an exact site origin (cache read only; 404 when it has none). */
  faviconBlob(origin: string): Promise<Blob>;
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
  connections(query?: ConnectionsQuery): Promise<ConnectionList>;
  /** Upserts the given agents' access without touching the others. */
  updateConnection(id: string, request: UpdateConnectionRequest): Promise<Connection>;
  /** Opens the site in a background tab and reads whether the sign-in still works. */
  checkConnection(id: string): Promise<Connection>;
  /** `signed_in` when the Vault finished it silently; `needs_you` carries the ticket the person completes. */
  reloginConnection(id: string): Promise<ReloginResult>;
  /** A separate persistent browser profile for one more account on `site`, with its sign-in started. */
  createAccountSpace(site: string): Promise<CreateAccountSpaceResult>;
  /** Forget (default) or, with `sign_out`, also clear the site's data in the agent browser. */
  revokeConnection(id: string, request: RevokeConnectionRequest): Promise<void>;
  dismissConnectionCandidate(id: string): Promise<void>;
  connectionAudit(query?: ConnectionAuditQuery): Promise<ConnectionAuditList>;
  /** First-run guide state, kept on the daemon so every device agrees. */
  onboarding(): Promise<OnboardingStatus>;
  /** Record how the guide ended; reopening it from Settings does not touch the flag. */
  setOnboarding(state: Exclude<OnboardingState, "pending">): Promise<OnboardingStatus>;
  externalAgents(): Promise<ExternalAgentList>;
  /** Add or change the skills folder of an agent that is not a preset (the computer's own authority). */
  setExternalAgent(id: string, skillsDir: string): Promise<ExternalAgent>;
  deleteExternalAgent(id: string): Promise<ExternalAgentList>;
  /** Install the bundled browser skill into the agent's skills directory; 409 `exists` unless `force`. */
  installAgentSkill(agent: string, force?: boolean): Promise<InstalledAgentSkill>;
  importProfiles(): Promise<ImportProfileList>;
  previewImport(request: ImportRequest): Promise<ImportBatchReport>;
  runImport(request: ImportRequest): Promise<ImportJob>;
  importJob(id: string): Promise<ImportJob>;
  importPasswordsCsv(csv: string): Promise<ImportCategoryReport>;
  listImportDelivery(): Promise<ImportDeliveryList>;
  mintImportDeliveryTicket(id: string, device: string): Promise<{ ticket: string }>;
  acknowledgeImportDelivery(id: string): Promise<void>;
  listDevices(): Promise<DeviceList>;
  revokeDevice(id: string): Promise<void>;
  /** Public route: the one-time code is the credential, so no bearer token is needed. */
  exchangePairing(code: string): Promise<PairDeviceResponse>;
  /** Admin, and only from the computer running the daemon: issues a one-time code for a phone or agent. */
  createPairing(request: CreatePairingRequest): Promise<PairingOffer>;
  /** This device's own record (scopes included) and its pending request for more access, if any. */
  myDevice(): Promise<MyDevice>;
  /** Any person's device: ask for scopes it lacks. Someone on the computer running openside approves it there. */
  requestAccess(scopes: CapabilityScope[]): Promise<AccessRequest>;
  /** Admin: requests waiting for a person on this computer. */
  accessRequests(): Promise<AccessRequestList>;
  /** Admin, and only from the computer running the daemon: grants the requested scopes. */
  approveAccessRequest(id: string): Promise<Device>;
  denyAccessRequest(id: string): Promise<void>;
  /** Transports, their live state and whether this caller may pair or reconfigure. */
  remoteStatus(): Promise<RemoteStatus>;
  setTailscale(enabled: boolean): Promise<RemoteStatus>;
  setCloudflare(request: SetCloudflareRequest): Promise<RemoteStatus>;
  setRelay(request: SetRelayRequest): Promise<RemoteStatus>;
  vapidKey(): Promise<VapidPublicKey>;
  listPushSubscriptions(): Promise<PushSubscriptionList>;
  createPushSubscription(request: CreatePushSubscriptionRequest): Promise<PushSubscription>;
  deletePushSubscription(id: string): Promise<void>;
  pushPresence(subscriptionId: string, focused: boolean): Promise<void>;
  /** Identity of the running process; `boot_id` changes on every start. */
  daemonStatus(): Promise<DaemonStatus>;
  /** Computer-only. 202: the restart is requested, not finished. */
  restartDaemon(): Promise<RestartDaemonResponse>;
  /** The daemon's own diagnostics, grouped by area, each finding with a fix the screen can offer. */
  doctor(): Promise<DoctorReport>;
  memoryModel(): Promise<MemoryModelStatus>;
  /** An empty request fetches the default model; naming a source needs the computer's authority. */
  downloadMemoryModel(request?: StartMemoryModelRequest): Promise<MemoryModelStatus>;
  cancelMemoryModel(): Promise<MemoryModelStatus>;
  policy(): Promise<PolicySettings>;
  /** Computer-only: the policy decides what agents may do. */
  setPolicy(policy: PolicySettings): Promise<PolicySettings>;
  testPolicy(request: PolicyTestRequest): Promise<PolicyDecision>;
  /** Checks an edit without saving; problems name the list and row to fix. */
  validatePolicy(request: ValidatePolicyRequest): Promise<PolicyValidation>;
  networkPolicy(): Promise<NetworkPolicy>;
  setNetworkPolicy(allow: string[]): Promise<NetworkPolicy>;
  syncStatus(): Promise<SyncStatus>;
  /** Passphrase and WebDAV password are typed here; the daemon keeps them in memory only. */
  setupSync(secrets: SyncSecrets): Promise<SyncSetupResponse>;
  pairSync(request: PairSyncRequest): Promise<SyncStatus>;
  unlockSync(secrets: SyncSecrets): Promise<SyncStatus>;
  runSync(): Promise<SyncReport>;
  setSyncConfig(request: UpdateSyncConfigRequest): Promise<SyncStatus>;
  pinnedTabs(): Promise<PinnedTabList>;
  pinTab(url: string, title?: string): Promise<PinnedTab>;
  unpinTab(id: string): Promise<void>;
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

  async function json<T>(method: string, path: string, body?: unknown, extra?: Record<string, string>): Promise<T> {
    const init: RequestInit = { method, headers: headers(extra) };
    if (body !== undefined) {
      init.headers = headers({ "Content-Type": "application/json", ...extra });
      init.body = JSON.stringify(body);
    }
    const response = await send(path, init);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  const enc = encodeURIComponent;

  function local(): Record<string, string> | undefined {
    const secret = options.relay === true ? null : options.localSecret?.();
    return secret ? { "X-Openside-Local-Secret": secret } : undefined;
  }

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
    modelCatalog: () => json<ModelCatalog>("GET", "/v1/settings/models"),
    setModels: (request: ModelsUpdate) => json<ModelsSaved>("PUT", "/v1/settings/models", request),
    listApiKeys: () => json<ProviderKeyList>("GET", "/v1/settings/keys"),
    addApiKey: (provider, request) => json<ProviderKeyInfo>("POST", `/v1/settings/keys/${enc(provider)}`, request),
    replaceApiKey: (provider, keyId, request) => json<ProviderKeyInfo>("PUT", `/v1/settings/keys/${enc(provider)}/${enc(keyId)}`, request),
    updateApiKey: (provider, keyId, patch) => json<ProviderKeyInfo>("PATCH", `/v1/settings/keys/${enc(provider)}/${enc(keyId)}`, patch),
    deleteApiKey: (provider, keyId) => json<void>("DELETE", `/v1/settings/keys/${enc(provider)}/${enc(keyId)}`),
    listSubscriptions: () => json<SubscriptionList>("GET", "/v1/settings/subscriptions"),
    addSubscriptionAccount: (source, label) => json<SubscriptionAccount>("POST", `/v1/settings/subscriptions/${enc(source)}`, { label }),
    subscriptionAccount: (source, accountId) => json<SubscriptionAccount>("GET", `/v1/settings/subscriptions/${enc(source)}/${enc(accountId)}`),
    updateSubscriptionAccount: (source, accountId, patch) =>
      json<SubscriptionAccount>("PATCH", `/v1/settings/subscriptions/${enc(source)}/${enc(accountId)}`, patch),
    deleteSubscriptionAccount: (source, accountId) => json<void>("DELETE", `/v1/settings/subscriptions/${enc(source)}/${enc(accountId)}`),
    listPools: () => json<ProviderPoolList>("GET", "/v1/settings/pools"),
    setPool: (provider, accounts) => json<ProviderPool>("PUT", `/v1/settings/pools/${enc(provider)}`, { accounts }),
    setSessionAccount: (id, account) => json<SessionAccountSelection>("PUT", `/v1/sessions/${enc(id)}/account`, { account }),
    listSessions: (query: ListSessionsQuery = {}) => {
      const params = new URLSearchParams();
      if (query.q?.trim()) params.set("q", query.q.trim());
      if (query.archived) params.set("archived", "true");
      if (query.agent) params.set("agent", query.agent);
      const suffix = params.size ? `?${params}` : "";
      return json<SessionList>("GET", `/v1/sessions${suffix}`);
    },
    listAgents: () => json<AgentList>("GET", "/v1/agents"),
    createAgent: (draft) => json<Agent>("POST", "/v1/agents", draft),
    updateAgent: (id, patch) => json<Agent>("PATCH", `/v1/agents/${enc(id)}`, patch),
    retireAgent: (id, request) => json<Agent>("POST", `/v1/agents/${enc(id)}/retire`, request),
    restoreAgent: (id) => json<Agent>("POST", `/v1/agents/${enc(id)}/restore`),
    suppressedProposals: () => json<SuppressedProposalList>("GET", "/v1/team/proposals/suppressed"),
    clearSuppressedProposal: async (key) => {
      await send(`/v1/team/proposals/${enc(key)}`, { method: "DELETE", headers: headers() });
    },
    channelAccounts: () => json<ChannelAccountList>("GET", "/v1/settings/channels"),
    addChannelAccount: (kind, request) => json<ChannelAccount>("POST", `/v1/settings/channels/${enc(kind)}/accounts`, request),
    updateChannelAccount: (kind, accountId, patch) => json<ChannelAccount>("PATCH", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}`, patch),
    deleteChannelAccount: (kind, accountId) => json<void>("DELETE", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}`),
    testChannelAccount: (kind, accountId) => json<ChannelAccountTest>("POST", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}/test`),
    createSession: (request: CreateSessionRequest) => json<Session>("POST", "/v1/sessions", request),
    updateSession: (id: string, request: UpdateSessionRequest) =>
      json<Session>("PATCH", `/v1/sessions/${enc(id)}`, request),
    deleteSession: (id: string) => json<void>("DELETE", `/v1/sessions/${enc(id)}`),
    stopSession: (id: string) => json<Session>("POST", `/v1/sessions/${enc(id)}/stop`),
    prewarmSession: async (id: string) => {
      await send(`/v1/sessions/${enc(id)}/prewarm`, { method: "POST", headers: headers() });
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
    resolveTicket: (id: string, value: unknown, extra?: Record<string, unknown>) =>
      json<Ticket>("POST", `/v1/tickets/${enc(id)}/resolve`, { value, ...extra }),
    rejectTicket: (id: string, reason?: string) =>
      json<Ticket>("POST", `/v1/tickets/${enc(id)}/reject`, { reason: reason ?? null }),
    ticketDone: (id: string) => json<Ticket>("POST", `/v1/tickets/${enc(id)}/done`),
    cancelSubagent: (id: string, subagentId: string) =>
      json<Subagent>("POST", `/v1/sessions/${enc(id)}/subagents/${enc(subagentId)}/cancel`),
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
    faviconBlob: async (origin: string) => {
      const response = await send(`/v1/favicons/${enc(origin)}`, { headers: headers() });
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
    setExternalAgent: (id, skillsDir) => json<ExternalAgent>("PUT", `/v1/external-agents/${enc(id)}`, { skills_dir: skillsDir }, local()),
    deleteExternalAgent: (id) => json<ExternalAgentList>("DELETE", `/v1/external-agents/${enc(id)}`, undefined, local()),
    installAgentSkill: (agent: string, force = false) =>
      json<InstalledAgentSkill>("POST", `/v1/external-agents/${enc(agent)}/browser-skill`, { force }),
    importProfiles: () => json<ImportProfileList>("GET", "/v1/import/profiles", undefined, local()),
    previewImport: (request: ImportRequest) => json<ImportBatchReport>("POST", "/v1/import/preview", request, local()),
    runImport: (request: ImportRequest) => json<ImportJob>("POST", "/v1/import", request, local()),
    importJob: (id: string) => json<ImportJob>("GET", `/v1/import/jobs/${enc(id)}`, undefined, local()),
    importPasswordsCsv: (csv: string) => json<ImportCategoryReport>("POST", "/v1/import/passwords", { csv }, local()),
    listImportDelivery: () => json<ImportDeliveryList>("GET", "/v1/import/delivery?target=user", undefined, local()),
    mintImportDeliveryTicket: (id: string, device: string) => json<{ ticket: string }>("POST", `/v1/import/delivery/${enc(id)}/ticket`, { device_id: device }, local()),
    acknowledgeImportDelivery: (id: string) => json<void>("POST", `/v1/import/delivery/${enc(id)}/ack`, { target: "user" }, local()),
    listDevices: () => json<DeviceList>("GET", "/v1/devices"),
    revokeDevice: (id: string) => json<void>("DELETE", `/v1/devices/${enc(id)}`),
    exchangePairing: (code: string) =>
      json<PairDeviceResponse>("POST", "/v1/pairing/exchange", { code }),
    createPairing: (request: CreatePairingRequest) => json<PairingOffer>("POST", "/v1/pairing", request, local()),
    myDevice: () => json<MyDevice>("GET", "/v1/devices/me"),
    requestAccess: (scopes: CapabilityScope[]) => json<AccessRequest>("POST", "/v1/devices/me/access-requests", { scopes }),
    accessRequests: () => json<AccessRequestList>("GET", "/v1/access-requests"),
    approveAccessRequest: (id: string) => json<Device>("POST", `/v1/access-requests/${enc(id)}/approve`, undefined, local()),
    denyAccessRequest: (id: string) => json<void>("POST", `/v1/access-requests/${enc(id)}/deny`),
    remoteStatus: () => json<RemoteStatus>("GET", "/v1/remote/status", undefined, local()),
    setTailscale: (enabled: boolean) => json<RemoteStatus>("PUT", "/v1/remote/tailscale", { enabled }, local()),
    setCloudflare: (request: SetCloudflareRequest) => json<RemoteStatus>("PUT", "/v1/remote/cloudflare", request, local()),
    setRelay: (request: SetRelayRequest) => json<RemoteStatus>("PUT", "/v1/remote/relay", request, local()),
    vapidKey: () => json<VapidPublicKey>("GET", "/v1/push/vapid"),
    listPushSubscriptions: () => json<PushSubscriptionList>("GET", "/v1/push/subscriptions"),
    createPushSubscription: (request: CreatePushSubscriptionRequest) =>
      json<PushSubscription>("POST", "/v1/push/subscriptions", request),
    deletePushSubscription: (id: string) => json<void>("DELETE", `/v1/push/subscriptions/${enc(id)}`),
    pushPresence: (subscriptionId: string, focused: boolean) =>
      json<void>("POST", "/v1/push/presence", { subscription_id: subscriptionId, focused }),
    daemonStatus: () => json<DaemonStatus>("GET", "/v1/daemon"),
    restartDaemon: () => json<RestartDaemonResponse>("POST", "/v1/daemon/restart", undefined, local()),
    doctor: () => json<DoctorReport>("GET", "/v1/doctor"),
    memoryModel: () => json<MemoryModelStatus>("GET", "/v1/memory/model"),
    downloadMemoryModel: (request = {}) => json<MemoryModelStatus>("POST", "/v1/memory/model/download", request, local()),
    cancelMemoryModel: () => json<MemoryModelStatus>("POST", "/v1/memory/model/cancel"),
    policy: () => json<PolicySettings>("GET", "/v1/policy"),
    setPolicy: (policy) => json<PolicySettings>("PUT", "/v1/policy", policy, local()),
    testPolicy: (request) => json<PolicyDecision>("POST", "/v1/policy/test", request),
    validatePolicy: (request) => json<PolicyValidation>("POST", "/v1/policy/validate", request),
    networkPolicy: () => json<NetworkPolicy>("GET", "/v1/policy/network"),
    setNetworkPolicy: (allow) => json<NetworkPolicy>("PUT", "/v1/policy/network", { allow }, local()),
    syncStatus: () => json<SyncStatus>("GET", "/v1/sync"),
    setupSync: (secrets) => json<SyncSetupResponse>("POST", "/v1/sync/setup", secrets, local()),
    pairSync: (request) => json<SyncStatus>("POST", "/v1/sync/pair", request, local()),
    unlockSync: (secrets) => json<SyncStatus>("POST", "/v1/sync/unlock", secrets, local()),
    runSync: () => json<SyncReport>("POST", "/v1/sync/run"),
    setSyncConfig: (request) => json<SyncStatus>("PUT", "/v1/sync/config", request, local()),
    pinnedTabs: () => json<PinnedTabList>("GET", "/v1/pinned-tabs"),
    pinTab: (url, title) => json<PinnedTab>("POST", "/v1/pinned-tabs", title ? { url, title } : { url }),
    unpinTab: (id) => json<void>("DELETE", `/v1/pinned-tabs/${enc(id)}`),
    connections: (query: ConnectionsQuery = {}) => {
      const params = new URLSearchParams();
      if (query.agent) params.set("agent", query.agent);
      if (query.status) params.set("status", query.status);
      const suffix = params.size ? `?${params}` : "";
      return json<ConnectionList>("GET", `/v1/connections${suffix}`);
    },
    updateConnection: (id, request) => json<Connection>("PATCH", `/v1/connections/${enc(id)}`, request),
    checkConnection: (id) => json<Connection>("POST", `/v1/connections/${enc(id)}/check`),
    reloginConnection: (id) => json<ReloginResult>("POST", `/v1/connections/${enc(id)}/relogin`, {}),
    createAccountSpace: (site) => json<CreateAccountSpaceResult>("POST", "/v1/connections/spaces", { site }),
    revokeConnection: (id, request) => json<void>("POST", `/v1/connections/${enc(id)}/revoke`, request),
    dismissConnectionCandidate: (id) => json<void>("POST", `/v1/connections/candidates/${enc(id)}/dismiss`),
    connectionAudit: (query: ConnectionAuditQuery = {}) => {
      const params = new URLSearchParams();
      if (query.connection) params.set("connection", query.connection);
      if (query.agent) params.set("agent", query.agent);
      if (query.action) params.set("action", query.action);
      if (query.limit) params.set("limit", String(query.limit));
      const suffix = params.size ? `?${params}` : "";
      return json<ConnectionAuditList>("GET", `/v1/connections/audit${suffix}`);
    },
  };
}
