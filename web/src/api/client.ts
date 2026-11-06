import type {
  ApiErrorBody,
  ArtifactList,
  BrowserTabList,
  CreatePersonBookmarkRequest,
  CreateProjectRequest,
  CreateSessionRequest,
  Health,
  Message,
  ModelAccountRef,
  ModelCatalog,
  ModelsSaved,
  ModelsUpdate,
  PickedFolder,
  Project,
  ProjectList,
  ProviderKeyInfo,
  ProviderPool,
  ProviderPoolList,
  Session,
  SessionList,
  SetSessionFolderRequest,
  Settings,
  SkillList,
  Subagent,
  OAuthMethod,
  OAuthProviderList,
  OAuthTransaction,
  SubmitMessageRequest,
  Ticket,
  TicketList,
  UpdateProjectRequest,
  UpdateSessionRequest,
  ForkSessionRequest,
  VisualList,
  CreateShareRequest,
  ShareLink,
  ShareLinkList,
  SharedChat,
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
  MemoryHealth,
  OnboardingState,
  OnboardingStatus,
  PairDeviceResponse,
  PairingOffer,
  RemoteDisabled,
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
  SkillDetail,
  UpdateRoutineRequest,
  UpdateSkillRequest,
  VapidPublicKey,
  CreateVaultItemRequest,
  VaultItem,
  VaultItemList,
  VaultPolicyOverride,
  VaultStatus,
  DaemonStatus,
  MemoryModelStatus,
  NetworkPolicy,
  PairSyncRequest,
  PersonBookmark,
  PersonBookmarkTree,
  PersonDownloadList,
  PersonExtensionList,
  PersonHistoryList,
  PersonRecentlyClosedList,
  PersonTabCommand,
  PersonTabCommandResult,
  ClearBrowsingDataRequest,
  PersonNativeTool,
  PersonTabList,
  PinnedTab,
  PinnedTabList,
  Preferences,
  PolicySettings,
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
  RetireAgentRequest,
  ChannelAccountList,
  ChannelAccount,
  ChannelAccountTest,
  AddChannelAccountRequest,
  UpdateChannelAccountRequest,
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
import type { CliDefaults, ComputerUseStatus, ComputerUseStopResult, UpdateCliDefaultsRequest, UpdatePreferencesRequest } from "./types";
import type { About, BrowserPrefs, UpdateBrowserPrefsRequest, UpdateCheck } from "./types";
import type { AttachedTab, AttachedTabList, McpServerList, SttStatus, TranscribeRequest, Transcript } from "./types";
import type { CreateMcpServerRequest, McpServer, ProviderConnectionList, UpdateMcpServerRequest } from "./types";
import type { UpdateVaultSettingsRequest, VaultBiometrics, VaultClearResult, VaultConfirmRequest, VaultExport, VaultRecoverRequest, VaultRecoveryKey, VaultSettings } from "./types";
import type { MemoryChangeList, MemoryFile, MemoryFileList, MemoryFileSaved, MemoryRebuild, SettingsDocument, UpdateMemoryFileRequest } from "./types";
import type { ImportDeliveryList } from "../import/protocol";
import { t, tMaybe } from "../i18n";

/** Explanation for a daemon error code that has a dedicated one; callers fall back to the daemon message. */
export function apiErrorMessage(code: string): string | undefined {
  return tMaybe(`api.error.${code}`);
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message?: string) {
    super(apiErrorMessage(code) ?? message ?? t("api.requestFailed", { status }));
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
  /** A project id lists that project's chats; `none` lists chats outside every project. */
  project?: string;
  /** Only the first N after ordering, for a "recent chats" view. */
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
  settings(): Promise<Settings>;
  /** Every model by provider with availability, plus the default, fallbacks and subscription logins. */
  modelCatalog(): Promise<ModelCatalog>;
  /** Connected providers, one row per provider and connection kind, for Settings › Models. */
  modelProviders(): Promise<ProviderConnectionList>;
  setModels(request: ModelsUpdate): Promise<ModelsSaved>;
  addApiKey(provider: string, request: { label: string; key: string }): Promise<ProviderKeyInfo>;
  replaceApiKey(provider: string, keyId: string, request: { key: string; label?: string }): Promise<ProviderKeyInfo>;
  deleteApiKey(provider: string, keyId: string): Promise<void>;
  deleteSubscriptionAccount(source: string, accountId: string): Promise<void>;
  /** Providers openside can sign in to itself, with their sign-in methods. */
  oauthProviders(): Promise<OAuthProviderList>;
  /** 202: starts a sign-in; the account exists only once the transaction is `ready`. */
  createOAuthAccount(provider: string, request: { label: string; join_pool: boolean; method?: OAuthMethod }): Promise<OAuthTransaction>;
  deleteOAuthAccount(provider: string, accountId: string): Promise<void>;
  reloginOAuthAccount(provider: string, accountId: string, request?: { method?: OAuthMethod }): Promise<OAuthTransaction>;
  /** Polled while the person signs in. */
  oauthTransaction(transactionId: string): Promise<OAuthTransaction>;
  /** The pasted code or final page address; write-only. */
  submitOAuthInput(transactionId: string, code: string): Promise<OAuthTransaction>;
  cancelOAuthTransaction(transactionId: string): Promise<void>;
  listPools(): Promise<ProviderPoolList>;
  setPool(provider: string, accounts: ModelAccountRef[]): Promise<ProviderPool>;
  /** `signal` cancels a search the person has already typed past. */
  listSessions(query?: ListSessionsQuery, signal?: AbortSignal): Promise<SessionList>;
  /** The team: the chief and every teammate, with live thread state for the sidebar. */
  listAgents(): Promise<AgentList>;
  retireAgent(id: string, request: RetireAgentRequest): Promise<Agent>;
  /** Messaging identities (Telegram, Slack, Discord) a teammate can answer on; no credentials. */
  channelAccounts(): Promise<ChannelAccountList>;
  /** Admin from a person's device or the local socket. Credentials are write-only and never come back. */
  addChannelAccount(kind: string, request: AddChannelAccountRequest): Promise<ChannelAccount>;
  /** Omitted fields stay; `options` replaces every option; an empty `agent_id` clears the assignment. */
  updateChannelAccount(kind: string, accountId: string, patch: Partial<UpdateChannelAccountRequest>): Promise<ChannelAccount>;
  deleteChannelAccount(kind: string, accountId: string): Promise<void>;
  /** A read-only identity check with the service; never posts a message. */
  testChannelAccount(kind: string, accountId: string): Promise<ChannelAccountTest>;
  /** One chat's options and context use; the timeline itself comes from the event stream. */
  session(id: string): Promise<Session>;
  createSession(request: CreateSessionRequest): Promise<Session>;
  updateSession(id: string, request: UpdateSessionRequest): Promise<Session>;
  /** Branches the chat at the finished reply to `from_message` into a new chat (201). */
  forkSession(id: string, request: ForkSessionRequest): Promise<Session>;
  deleteSession(id: string): Promise<void>;
  /**
   * Moves the chat into a project, attaches a folder of its own, or (both null) leaves both. A
   * folder can only be attached from the computer running openside (403 local_access_required).
   */
  setSessionFolder(id: string, request: SetSessionFolderRequest): Promise<Session>;
  /** Every project, by name. */
  listProjects(): Promise<ProjectList>;
  /** Only from the computer running openside; without `root` the daemon makes an empty folder. */
  createProject(request: CreateProjectRequest): Promise<Project>;
  /** Name and instructions only; the folder is fixed at creation. */
  updateProject(id: string, request: UpdateProjectRequest): Promise<Project>;
  /** The project's chats leave it; its folder stays on disk. Returns the projects left. */
  deleteProject(id: string): Promise<ProjectList>;
  /** Opens the folder picker on the computer running openside; `path` is null when cancelled. */
  pickFolder(): Promise<PickedFolder>;
  stopSession(id: string): Promise<Session>;
  /** Start the session's agent process, browser and a blank agent tab ahead of a message. */
  prewarmSession(id: string): Promise<void>;
  /** The person's tabs the chat's agent may read; answers from the daemon's memory, so it works without the extension. */
  attachedTabs(id: string): Promise<AttachedTabList>;
  /** Without `tabId` the person's active tab is attached; 409 person_browser_not_connected without the extension. */
  attachTab(id: string, tabId?: number): Promise<AttachedTab>;
  detachTab(id: string, tabId: number): Promise<void>;
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
  /** `/` mention candidates: enabled skills, scoped to the session when there is one. */
  searchSkills(sessionId: string | undefined, q: string): Promise<SkillList>;
  mcpServers(): Promise<McpServerList>;
  createMcpServer(request: CreateMcpServerRequest): Promise<McpServer>;
  updateMcpServer(id: string, patch: UpdateMcpServerRequest): Promise<McpServer>;
  deleteMcpServer(id: string): Promise<McpServer>;
  /** Reconnects and lists the tools again; needs local access because it runs a stdio server's program. */
  refreshMcpServer(id: string): Promise<McpServer>;
  /** Whether the composer's microphone can be transcribed; person devices only. */
  sttStatus(): Promise<SttStatus>;
  /**
   * One recorded clip to text; the daemon keeps neither. 409 stt_unavailable, 413 audio_too_large,
   * 422 no_speech. `signal` drops a clip the person stopped waiting for (Stop while transcribing,
   * or the composer went away).
   */
  transcribe(request: TranscribeRequest, signal?: AbortSignal): Promise<Transcript>;
  listArtifacts(id: string): Promise<ArtifactList>;
  listVisuals(id: string): Promise<VisualList>;
  /** Raw artifact bytes through the authenticated download route. */
  artifactBlob(id: string, path: string): Promise<Blob>;
  listShares(id: string): Promise<ShareLinkList>;
  /** Snapshot the chat as it is now; only a person can publish one. */
  createShare(id: string, request: CreateShareRequest): Promise<ShareLink>;
  revokeShare(id: string, shareId: string): Promise<void>;
  /** The public snapshot behind a link; sent without a token, since the id is the credential. */
  sharedChat(shareId: string): Promise<SharedChat>;
  sharedArtifactBlob(shareId: string, index: number): Promise<Blob>;
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
  /**
   * The person's own control of one agent tab, over the takeover protocol plus address bar
   * commands; the agent is locked out of the tab until the socket closes.
   */
  browserControl(sessionId: string, tabId: string): WebSocket;
  browserThumbnail(sessionId: string, thumbnailId: string, signal?: AbortSignal): Promise<Blob>;
  /** Agent-owned, non-private tabs with sanitized URLs; 503 `browser_disconnected` when the controller is gone. */
  browserTabs(): Promise<BrowserTabList>;
  /** The person's own browser through the paired extension; `connected: false` with empty lists while it is not attached. */
  personTabs(): Promise<PersonTabList>;
  /** Mutations answer 409 `person_browser_not_connected` while the extension is not attached. */
  personTabCommand(command: PersonTabCommand): Promise<PersonTabCommandResult>;
  /** 409 `person_browser_not_connected` while the extension is not attached; `since_ms: 0` clears all time. */
  clearBrowsingData(request: ClearBrowsingDataRequest): Promise<void>;
  /** Opens the Task manager or DevTools in the person's own browser. 409 `browser_not_running` while it is closed; 409 `native_tools_unsupported` for a stock browser or when a policy turns DevTools off. */
  openBrowserTool(tool: PersonNativeTool): Promise<void>;
  personBookmarks(signal?: AbortSignal): Promise<PersonBookmarkTree>;
  createPersonBookmark(request: CreatePersonBookmarkRequest): Promise<PersonBookmark>;
  deletePersonBookmark(id: string): Promise<void>;
  personHistory(query?: { q?: string; limit?: number }, signal?: AbortSignal): Promise<PersonHistoryList>;
  personDownloads(limit?: number): Promise<PersonDownloadList>;
  personExtensions(signal?: AbortSignal): Promise<PersonExtensionList>;
  /** Recently closed tabs and windows, newest first (daemon default 10, at most 25). */
  personRecentlyClosed(limit?: number, signal?: AbortSignal): Promise<PersonRecentlyClosedList>;
  /** Reopens one; 409 `person_browser_not_connected` while the extension is not attached. */
  restorePersonRecentlyClosed(sessionId: string): Promise<PersonTabCommandResult>;
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
  memoryHealth(): Promise<MemoryHealth>;
  /** MEMORY.md, USER.md and TAXONOMY.md: the memory store as three files. */
  memoryFiles(): Promise<MemoryFileList>;
  memoryFile(name: string): Promise<MemoryFile>;
  /** 409 memory_conflict when a note changed after `revision`; TAXONOMY.md is read-only. */
  updateMemoryFile(name: string, request: UpdateMemoryFileRequest): Promise<MemoryFileSaved>;
  /** Every memory change, newest first; pass `next_before` as `before` for older ones. */
  memoryChanges(query?: { limit?: number; before?: number }): Promise<MemoryChangeList>;
  rebuildMemory(): Promise<MemoryRebuild>;
  /** Writes a copy of the three files and opens it in this computer's file manager; 403 local_access_required elsewhere. */
  revealMemoryFolder(): Promise<void>;
  /** The whole config.toml as JSON; changes apply when the daemon restarts. */
  settingsConfig(): Promise<SettingsDocument>;
  updateSettingsConfig(config: unknown): Promise<SettingsDocument>;
  vaultStatus(): Promise<VaultStatus>;
  /**
   * The passphrase is only for a passphrase vault; a keyring vault takes none. `biometric` asks
   * this computer's fingerprint or Touch ID check first, which a keyring vault with biometric
   * unlock needs; only the computer's own browser can do it.
   */
  setupVault(passphrase?: string, biometric?: boolean): Promise<VaultStatus>;
  unlockVault(passphrase?: string, biometric?: boolean): Promise<VaultStatus>;
  lockVault(): Promise<VaultStatus>;
  createVaultItem(request: CreateVaultItemRequest): Promise<VaultItem>;
  deleteVaultItem(id: string): Promise<void>;
  vaultItems(): Promise<VaultItemList>;
  updateVaultItem(id: string, policy: VaultPolicyOverride): Promise<VaultItem>;
  vaultSettings(): Promise<VaultSettings>;
  /** Loosening a setting needs a person's device; turning biometric unlock on needs this computer. */
  updateVaultSettings(patch: UpdateVaultSettingsRequest): Promise<VaultSettings>;
  vaultBiometrics(): Promise<VaultBiometrics>;
  /** The next four need this computer (403 local_access_required elsewhere) and `confirm: true`. */
  exportVault(request: VaultConfirmRequest): Promise<VaultExport>;
  clearVault(request: VaultConfirmRequest): Promise<VaultClearResult>;
  /** The key is in this response only; the daemon keeps no copy it could show again. */
  createVaultRecoveryKey(request: VaultConfirmRequest): Promise<VaultRecoveryKey>;
  recoverVault(request: VaultRecoverRequest): Promise<VaultStatus>;
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
  /** Admin; an agent-client token may rename only itself. */
  renameDevice(id: string, name: string): Promise<Device>;
  /** Public route: the one-time code is the credential, so no bearer token is needed. */
  exchangePairing(code: string): Promise<PairDeviceResponse>;
  /** Admin, and only from the computer running the daemon: issues a one-time code for a phone or agent. */
  createPairing(request: CreatePairingRequest): Promise<PairingOffer>;
  /** This device's own record (scopes included) and its pending request for more access, if any. */
  myDevice(): Promise<MyDevice>;
  preferences(): Promise<Preferences>;
  /** `name: ""` clears the person's name. `mcp_server` and `computer_use` need local access. */
  updatePreferences(patch: UpdatePreferencesRequest): Promise<Preferences>;
  /** The browser profile new CLI and MCP sessions on this computer use. */
  cliDefaults(): Promise<CliDefaults>;
  /** Local access only; `profile: null` goes back to the account's first profile. */
  updateCliDefaults(patch: UpdateCliDefaultsRequest): Promise<CliDefaults>;
  /** Whether this computer can run computer use, and whether the person turned it on. */
  computerUse(): Promise<ComputerUseStatus>;
  /** Ends the chat's desktop connection at once, cancelling an action in flight. */
  stopComputerUse(sessionId: string): Promise<ComputerUseStopResult>;
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
  /** Local person only: stops every connection, keeps its settings, and lists what could not be stopped. */
  disableRemote(): Promise<RemoteDisabled>;
  /** Local person only: turns Remote Control back on with the saved connections. */
  enableRemote(): Promise<RemoteStatus>;
  vapidKey(): Promise<VapidPublicKey>;
  listPushSubscriptions(): Promise<PushSubscriptionList>;
  createPushSubscription(request: CreatePushSubscriptionRequest): Promise<PushSubscription>;
  deletePushSubscription(id: string): Promise<void>;
  pushPresence(subscriptionId: string, focused: boolean): Promise<void>;
  /** Identity of the running process; `boot_id` changes on every start. */
  daemonStatus(): Promise<DaemonStatus>;
  /** Computer-only. 202: the restart is requested, not finished. */
  restartDaemon(): Promise<RestartDaemonResponse>;
  memoryModel(): Promise<MemoryModelStatus>;
  /** An empty request fetches the default model; naming a source needs the computer's authority. */
  downloadMemoryModel(request?: StartMemoryModelRequest): Promise<MemoryModelStatus>;
  cancelMemoryModel(): Promise<MemoryModelStatus>;
  policy(): Promise<PolicySettings>;
  /** Computer-only: the policy decides what agents may do. */
  setPolicy(policy: PolicySettings): Promise<PolicySettings>;
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
  /** Installed background service version and where releases are published. */
  about(): Promise<About>;
  /** Asks the release feed for a newer version; the only request that leaves the computer, so only on a click. */
  checkForUpdates(): Promise<UpdateCheck>;
  /** The person's browser settings; `status` says whether they can be read and changed right now. */
  browserPrefs(): Promise<BrowserPrefs>;
  /** Writes the given browser settings and returns the values read back. */
  updateBrowserPrefs(patch: UpdateBrowserPrefsRequest): Promise<BrowserPrefs>;
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

  /** A cancellable GET: the palette drops a read the person has typed past or closed. */
  async function getJson<T>(path: string, signal?: AbortSignal): Promise<T> {
    const response = await send(path, { headers: headers(), signal });
    return (await response.json()) as T;
  }

  const enc = encodeURIComponent;

  function local(): Record<string, string> | undefined {
    const secret = options.relay === true ? null : options.localSecret?.();
    return secret ? { "X-Openside-Local-Secret": secret } : undefined;
  }

  function unlockBody(passphrase: string | undefined, biometric: boolean): { passphrase?: string; biometric?: true } {
    return { ...(passphrase ? { passphrase } : {}), ...(biometric ? { biometric: true as const } : {}) };
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
    settings: () => json<Settings>("GET", "/v1/settings"),
    modelCatalog: () => json<ModelCatalog>("GET", "/v1/settings/models"),
    modelProviders: () => json<ProviderConnectionList>("GET", "/v1/settings/models/providers"),
    setModels: (request: ModelsUpdate) => json<ModelsSaved>("PUT", "/v1/settings/models", request),
    addApiKey: (provider, request) => json<ProviderKeyInfo>("POST", `/v1/settings/keys/${enc(provider)}`, request),
    replaceApiKey: (provider, keyId, request) => json<ProviderKeyInfo>("PUT", `/v1/settings/keys/${enc(provider)}/${enc(keyId)}`, request),
    deleteApiKey: (provider, keyId) => json<void>("DELETE", `/v1/settings/keys/${enc(provider)}/${enc(keyId)}`),
    deleteSubscriptionAccount: (source, accountId) => json<void>("DELETE", `/v1/settings/subscriptions/${enc(source)}/${enc(accountId)}`),
    oauthProviders: () => json<OAuthProviderList>("GET", "/v1/settings/oauth/providers"),
    createOAuthAccount: (provider, request) => json<OAuthTransaction>("POST", `/v1/settings/oauth/accounts/${enc(provider)}`, request),
    deleteOAuthAccount: (provider, accountId) => json<void>("DELETE", `/v1/settings/oauth/accounts/${enc(provider)}/${enc(accountId)}`),
    reloginOAuthAccount: (provider, accountId, request = {}) =>
      json<OAuthTransaction>("POST", `/v1/settings/oauth/accounts/${enc(provider)}/${enc(accountId)}/relogin`, request),
    oauthTransaction: (transactionId) => json<OAuthTransaction>("GET", `/v1/settings/oauth/transactions/${enc(transactionId)}`),
    submitOAuthInput: (transactionId, code) => json<OAuthTransaction>("POST", `/v1/settings/oauth/transactions/${enc(transactionId)}/input`, { code }),
    cancelOAuthTransaction: (transactionId) => json<void>("POST", `/v1/settings/oauth/transactions/${enc(transactionId)}/cancel`),
    listPools: () => json<ProviderPoolList>("GET", "/v1/settings/pools"),
    setPool: (provider, accounts) => json<ProviderPool>("PUT", `/v1/settings/pools/${enc(provider)}`, { accounts }),
    listSessions: async (query: ListSessionsQuery = {}, signal?: AbortSignal) => {
      const params = new URLSearchParams();
      if (query.q?.trim()) params.set("q", query.q.trim());
      if (query.archived) params.set("archived", "true");
      if (query.agent) params.set("agent", query.agent);
      if (query.project) params.set("project", query.project);
      if (query.limit !== undefined) params.set("limit", String(query.limit));
      const suffix = params.size ? `?${params}` : "";
      const response = await send(`/v1/sessions${suffix}`, { headers: headers(), signal });
      return (await response.json()) as SessionList;
    },
    listAgents: () => json<AgentList>("GET", "/v1/agents"),
    retireAgent: (id, request) => json<Agent>("POST", `/v1/agents/${enc(id)}/retire`, request),
    channelAccounts: () => json<ChannelAccountList>("GET", "/v1/settings/channels"),
    addChannelAccount: (kind, request) => json<ChannelAccount>("POST", `/v1/settings/channels/${enc(kind)}/accounts`, request),
    updateChannelAccount: (kind, accountId, patch) => json<ChannelAccount>("PATCH", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}`, patch),
    deleteChannelAccount: (kind, accountId) => json<void>("DELETE", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}`),
    testChannelAccount: (kind, accountId) => json<ChannelAccountTest>("POST", `/v1/settings/channels/${enc(kind)}/accounts/${enc(accountId)}/test`),
    // Local-aware: a folder named in the request is accepted only from the computer running openside.
    createSession: (request: CreateSessionRequest) => json<Session>("POST", "/v1/sessions", request, local()),
    session: (id: string) => json<Session>("GET", `/v1/sessions/${enc(id)}`),
    updateSession: (id: string, request: UpdateSessionRequest) =>
      json<Session>("PATCH", `/v1/sessions/${enc(id)}`, request),
    forkSession: (id: string, request: ForkSessionRequest) => json<Session>("POST", `/v1/sessions/${enc(id)}/fork`, request),
    deleteSession: (id: string) => json<void>("DELETE", `/v1/sessions/${enc(id)}`),
    setSessionFolder: (id, request) => json<Session>("PUT", `/v1/sessions/${enc(id)}/folder`, request, local()),
    listProjects: () => json<ProjectList>("GET", "/v1/projects"),
    createProject: (request) => json<Project>("POST", "/v1/projects", request, local()),
    updateProject: (id, request) => json<Project>("PATCH", `/v1/projects/${enc(id)}`, request),
    deleteProject: (id) => json<ProjectList>("DELETE", `/v1/projects/${enc(id)}`),
    pickFolder: () => json<PickedFolder>("POST", "/v1/folders/pick", undefined, local()),
    stopSession: (id: string) => json<Session>("POST", `/v1/sessions/${enc(id)}/stop`),
    prewarmSession: async (id: string) => {
      await send(`/v1/sessions/${enc(id)}/prewarm`, { method: "POST", headers: headers() });
    },
    attachedTabs: (id: string) => json<AttachedTabList>("GET", `/v1/sessions/${enc(id)}/attached-tabs`),
    attachTab: (id: string, tabId?: number) =>
      json<AttachedTab>("POST", `/v1/sessions/${enc(id)}/attached-tabs`, tabId === undefined ? {} : { tab_id: tabId }),
    detachTab: (id: string, tabId: number) =>
      json<void>("DELETE", `/v1/sessions/${enc(id)}/attached-tabs/${enc(String(tabId))}`),
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
    searchSkills: (sessionId: string | undefined, q: string) => {
      const params = new URLSearchParams({ q, enabled: "true" });
      if (sessionId) params.set("session", sessionId);
      return json<SkillList>("GET", `/v1/skills?${params}`);
    },
    mcpServers: () => json<McpServerList>("GET", "/v1/mcp/servers"),
    createMcpServer: (request: CreateMcpServerRequest) => json<McpServer>("POST", "/v1/mcp/servers", request, local()),
    updateMcpServer: (id: string, patch: UpdateMcpServerRequest) => json<McpServer>("PATCH", `/v1/mcp/servers/${enc(id)}`, patch, local()),
    deleteMcpServer: (id: string) => json<McpServer>("DELETE", `/v1/mcp/servers/${enc(id)}`, undefined, local()),
    refreshMcpServer: (id: string) => json<McpServer>("POST", `/v1/mcp/servers/${enc(id)}/refresh`, undefined, local()),
    sttStatus: () => json<SttStatus>("GET", "/v1/stt"),
    transcribe: async (request: TranscribeRequest, signal?: AbortSignal) => {
      const response = await send("/v1/stt", {
        method: "POST",
        headers: headers({ "Content-Type": "application/json" }),
        body: JSON.stringify(request),
        signal,
      });
      return (await response.json()) as Transcript;
    },
    listArtifacts: (id: string) => json<ArtifactList>("GET", `/v1/sessions/${enc(id)}/artifacts`),
    listVisuals: (id: string) => json<VisualList>("GET", `/v1/sessions/${enc(id)}/visuals`),
    artifactBlob: async (id: string, path: string) => {
      // The artifact path is one percent-encoded segment (`/` as %2F).
      const response = await send(`/v1/sessions/${enc(id)}/artifacts/${enc(path)}`, { headers: headers() });
      return response.blob();
    },
    listShares: (id: string) => json<ShareLinkList>("GET", `/v1/sessions/${enc(id)}/shares`),
    createShare: (id: string, request: CreateShareRequest) => json<ShareLink>("POST", `/v1/sessions/${enc(id)}/shares`, request),
    revokeShare: (id: string, shareId: string) => json<void>("DELETE", `/v1/sessions/${enc(id)}/shares/${enc(shareId)}`),
    sharedChat: async (shareId: string) => {
      const response = await send(`/v1/shares/${enc(shareId)}`, { headers: { Accept: "application/json" } });
      return (await response.json()) as SharedChat;
    },
    sharedArtifactBlob: async (shareId: string, index: number) => {
      const response = await send(`/v1/shares/${enc(shareId)}/artifacts/${index}`);
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
    browserControl: (sessionId: string, tabId: string) => openWebSocket(`/v1/sessions/${enc(sessionId)}/browser/tabs/${enc(tabId)}/control`),
    browserThumbnail: async (sessionId: string, thumbnailId: string, signal?: AbortSignal) => {
      const response = await send(`/v1/sessions/${enc(sessionId)}/browser/thumbnails/${enc(thumbnailId)}`, {
        headers: headers({ Accept: "image/jpeg" }), signal,
      });
      return response.blob();
    },
    browserTabs: () => json<BrowserTabList>("GET", "/v1/browser/tabs"),
    personTabs: () => json<PersonTabList>("GET", "/v1/browser/person/tabs"),
    personTabCommand: (command: PersonTabCommand) => json<PersonTabCommandResult>("POST", "/v1/browser/person/tabs/command", command),
    clearBrowsingData: (request: ClearBrowsingDataRequest) => json<void>("POST", "/v1/browser/person/clear-data", request),
    openBrowserTool: (tool: PersonNativeTool) => json<void>("POST", "/v1/browser/person/native-tools", { tool }),
    personBookmarks: (signal?: AbortSignal) => getJson<PersonBookmarkTree>("/v1/browser/person/bookmarks", signal),
    createPersonBookmark: (request: CreatePersonBookmarkRequest) => json<PersonBookmark>("POST", "/v1/browser/person/bookmarks", request),
    deletePersonBookmark: (id: string) => json<void>("DELETE", `/v1/browser/person/bookmarks/${enc(id)}`),
    personHistory: (query = {}, signal?: AbortSignal) => {
      const params = new URLSearchParams();
      if (query.q?.trim()) params.set("q", query.q.trim());
      if (query.limit !== undefined) params.set("limit", String(query.limit));
      return getJson<PersonHistoryList>(`/v1/browser/person/history${params.size ? `?${params}` : ""}`, signal);
    },
    personDownloads: (limit?: number) => json<PersonDownloadList>("GET", `/v1/browser/person/downloads${limit === undefined ? "" : `?limit=${limit}`}`),
    personExtensions: (signal?: AbortSignal) => getJson<PersonExtensionList>("/v1/browser/person/extensions", signal),
    personRecentlyClosed: (limit?: number, signal?: AbortSignal) => getJson<PersonRecentlyClosedList>(`/v1/browser/person/recently-closed${limit === undefined ? "" : `?limit=${limit}`}`, signal),
    restorePersonRecentlyClosed: (sessionId: string) => json<PersonTabCommandResult>("POST", `/v1/browser/person/recently-closed/${enc(sessionId)}/restore`),
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
    memoryHealth: () => json<MemoryHealth>("GET", "/v1/memory/health"),
    memoryFiles: () => json<MemoryFileList>("GET", "/v1/memory/files"),
    memoryFile: (name: string) => json<MemoryFile>("GET", `/v1/memory/files/${enc(name)}`),
    updateMemoryFile: (name: string, request: UpdateMemoryFileRequest) =>
      json<MemoryFileSaved>("PUT", `/v1/memory/files/${enc(name)}`, request),
    memoryChanges: (query?: { limit?: number; before?: number }) => {
      const params = new URLSearchParams();
      if (query?.limit !== undefined) params.set("limit", String(query.limit));
      if (query?.before !== undefined) params.set("before", String(query.before));
      const suffix = params.size ? `?${params}` : "";
      return json<MemoryChangeList>("GET", `/v1/memory/history${suffix}`);
    },
    rebuildMemory: () => json<MemoryRebuild>("POST", "/v1/memory/rebuild"),
    revealMemoryFolder: () => json<void>("POST", "/v1/memory/reveal", undefined, local()),
    settingsConfig: () => json<SettingsDocument>("GET", "/v1/settings/config", undefined, local()),
    updateSettingsConfig: (config: unknown) => json<SettingsDocument>("PUT", "/v1/settings/config", { config }, local()),
    vaultStatus: () => json<VaultStatus>("GET", "/v1/vault"),
    setupVault: (passphrase?: string, biometric = false) => json<VaultStatus>("POST", "/v1/vault/setup", unlockBody(passphrase, biometric), biometric ? local() : undefined),
    unlockVault: (passphrase?: string, biometric = false) => json<VaultStatus>("POST", "/v1/vault/unlock", unlockBody(passphrase, biometric), biometric ? local() : undefined),
    lockVault: () => json<VaultStatus>("POST", "/v1/vault/lock"),
    createVaultItem: (request: CreateVaultItemRequest) => json<VaultItem>("POST", "/v1/vault/items", request),
    deleteVaultItem: (id: string) => json<void>("DELETE", `/v1/vault/items/${enc(id)}`),
    vaultItems: () => json<VaultItemList>("GET", "/v1/vault/items"),
    updateVaultItem: (id: string, policy: VaultPolicyOverride) =>
      json<VaultItem>("PATCH", `/v1/vault/items/${enc(id)}`, { policy }),
    vaultSettings: () => json<VaultSettings>("GET", "/v1/vault/settings"),
    updateVaultSettings: (patch: UpdateVaultSettingsRequest) => json<VaultSettings>("PATCH", "/v1/vault/settings", patch, local()),
    vaultBiometrics: () => json<VaultBiometrics>("GET", "/v1/vault/biometrics"),
    exportVault: (request: VaultConfirmRequest) => json<VaultExport>("POST", "/v1/vault/export", request, local()),
    clearVault: (request: VaultConfirmRequest) => json<VaultClearResult>("POST", "/v1/vault/clear", request, local()),
    createVaultRecoveryKey: (request: VaultConfirmRequest) => json<VaultRecoveryKey>("POST", "/v1/vault/recovery-key", request, local()),
    recoverVault: (request: VaultRecoverRequest) => json<VaultStatus>("POST", "/v1/vault/recover", request, local()),
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
    renameDevice: (id: string, name: string) => json<Device>("PATCH", `/v1/devices/${enc(id)}`, { name }),
    exchangePairing: (code: string) =>
      json<PairDeviceResponse>("POST", "/v1/pairing/exchange", { code }),
    createPairing: (request: CreatePairingRequest) => json<PairingOffer>("POST", "/v1/pairing", request, local()),
    myDevice: () => json<MyDevice>("GET", "/v1/devices/me"),
    preferences: () => json<Preferences>("GET", "/v1/settings/preferences"),
    updatePreferences: (patch: UpdatePreferencesRequest) => json<Preferences>("PATCH", "/v1/settings/preferences", patch, local()),
    cliDefaults: () => json<CliDefaults>("GET", "/v1/settings/cli"),
    updateCliDefaults: (patch: UpdateCliDefaultsRequest) => json<CliDefaults>("PATCH", "/v1/settings/cli", patch, local()),
    computerUse: () => json<ComputerUseStatus>("GET", "/v1/computer-use"),
    stopComputerUse: (sessionId: string) => json<ComputerUseStopResult>("POST", `/v1/sessions/${enc(sessionId)}/computer-use/stop`),
    requestAccess: (scopes: CapabilityScope[]) => json<AccessRequest>("POST", "/v1/devices/me/access-requests", { scopes }),
    accessRequests: () => json<AccessRequestList>("GET", "/v1/access-requests"),
    approveAccessRequest: (id: string) => json<Device>("POST", `/v1/access-requests/${enc(id)}/approve`, undefined, local()),
    denyAccessRequest: (id: string) => json<void>("POST", `/v1/access-requests/${enc(id)}/deny`),
    remoteStatus: () => json<RemoteStatus>("GET", "/v1/remote/status", undefined, local()),
    setTailscale: (enabled: boolean) => json<RemoteStatus>("PUT", "/v1/remote/tailscale", { enabled }, local()),
    setCloudflare: (request: SetCloudflareRequest) => json<RemoteStatus>("PUT", "/v1/remote/cloudflare", request, local()),
    setRelay: (request: SetRelayRequest) => json<RemoteStatus>("PUT", "/v1/remote/relay", request, local()),
    disableRemote: () => json<RemoteDisabled>("POST", "/v1/remote/disable", undefined, local()),
    enableRemote: () => json<RemoteStatus>("POST", "/v1/remote/enable", undefined, local()),
    vapidKey: () => json<VapidPublicKey>("GET", "/v1/push/vapid"),
    listPushSubscriptions: () => json<PushSubscriptionList>("GET", "/v1/push/subscriptions"),
    createPushSubscription: (request: CreatePushSubscriptionRequest) =>
      json<PushSubscription>("POST", "/v1/push/subscriptions", request),
    deletePushSubscription: (id: string) => json<void>("DELETE", `/v1/push/subscriptions/${enc(id)}`),
    pushPresence: (subscriptionId: string, focused: boolean) =>
      json<void>("POST", "/v1/push/presence", { subscription_id: subscriptionId, focused }),
    daemonStatus: () => json<DaemonStatus>("GET", "/v1/daemon"),
    restartDaemon: () => json<RestartDaemonResponse>("POST", "/v1/daemon/restart", undefined, local()),
    memoryModel: () => json<MemoryModelStatus>("GET", "/v1/memory/model"),
    downloadMemoryModel: (request = {}) => json<MemoryModelStatus>("POST", "/v1/memory/model/download", request, local()),
    cancelMemoryModel: () => json<MemoryModelStatus>("POST", "/v1/memory/model/cancel"),
    policy: () => json<PolicySettings>("GET", "/v1/policy"),
    setPolicy: (policy) => json<PolicySettings>("PUT", "/v1/policy", policy, local()),
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
    about: () => json<About>("GET", "/v1/about"),
    checkForUpdates: () => json<UpdateCheck>("POST", "/v1/about/update-check"),
    browserPrefs: () => json<BrowserPrefs>("GET", "/v1/browser/prefs"),
    updateBrowserPrefs: (patch) => json<BrowserPrefs>("PATCH", "/v1/browser/prefs", patch),
  };
}
