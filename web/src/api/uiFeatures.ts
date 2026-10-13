/**
 * PP-16 / E-4 / G-5 registry: every web UI feature and the public API calls it is built from.
 * `uiApiMap.test.ts` proves each call really hits the endpoint written here, that the calls
 * appear in the listed source files, and that every OpenAPI endpoint is either used or listed
 * in UNSURFACED with a reason. `docs/specs/ui-api-map.md` is rendered from this file by
 * `pnpm -C web gen:ui-map`.
 *
 * Type-only imports keep this file runnable by Node's type stripping (the generator script).
 */
import type { Client } from "./client";
import type { paths } from "./schema";

type Verb = "get" | "put" | "post" | "delete" | "patch";

/** `METHOD /path/{template}` for every operation in the generated OpenAPI types. */
export type Endpoint = {
  [P in keyof paths]: {
    [V in Verb]: paths[P][V] extends undefined ? never : `${Uppercase<V>} ${P & string}`;
  }[Verb];
}[keyof paths];

/** Client members that are helpers rather than requests. */
type Helper = "baseUrl" | "relay" | "headers" | "fetch" | "resolve";

/** Every daemon call goes through a typed client method or the SSE reader. */
export type CallName = Exclude<keyof Client, Helper> | "openSessionStream" | "pollSessionEvents";

export const CALLS: Record<CallName, Endpoint> = {
  health: "GET /v1/health",
  accounts: "GET /v1/accounts",
  settings: "GET /v1/settings",
  modelCatalog: "GET /v1/settings/models",
  setModels: "PUT /v1/settings/models",
  listApiKeys: "GET /v1/settings/keys",
  addApiKey: "POST /v1/settings/keys/{provider}",
  replaceApiKey: "PUT /v1/settings/keys/{provider}/{key_id}",
  updateApiKey: "PATCH /v1/settings/keys/{provider}/{key_id}",
  deleteApiKey: "DELETE /v1/settings/keys/{provider}/{key_id}",
  listSubscriptions: "GET /v1/settings/subscriptions",
  addSubscriptionAccount: "POST /v1/settings/subscriptions/{source}",
  subscriptionAccount: "GET /v1/settings/subscriptions/{source}/{account_id}",
  updateSubscriptionAccount: "PATCH /v1/settings/subscriptions/{source}/{account_id}",
  deleteSubscriptionAccount: "DELETE /v1/settings/subscriptions/{source}/{account_id}",
  listPools: "GET /v1/settings/pools",
  setPool: "PUT /v1/settings/pools/{provider}",
  setSessionAccount: "PUT /v1/sessions/{id}/account",
  listSessions: "GET /v1/sessions",
  listAgents: "GET /v1/agents",
  createAgent: "POST /v1/agents",
  updateAgent: "PATCH /v1/agents/{id}",
  retireAgent: "POST /v1/agents/{id}/retire",
  restoreAgent: "POST /v1/agents/{id}/restore",
  suppressedProposals: "GET /v1/team/proposals/suppressed",
  clearSuppressedProposal: "DELETE /v1/team/proposals/{key}",
  channelAccounts: "GET /v1/settings/channels",
  addChannelAccount: "POST /v1/settings/channels/{kind}/accounts",
  updateChannelAccount: "PATCH /v1/settings/channels/{kind}/accounts/{account_id}",
  deleteChannelAccount: "DELETE /v1/settings/channels/{kind}/accounts/{account_id}",
  testChannelAccount: "POST /v1/settings/channels/{kind}/accounts/{account_id}/test",
  createSession: "POST /v1/sessions",
  updateSession: "PATCH /v1/sessions/{id}",
  deleteSession: "DELETE /v1/sessions/{id}",
  stopSession: "POST /v1/sessions/{id}/stop",
  prewarmSession: "POST /v1/sessions/{id}/prewarm",
  submitMessage: "POST /v1/sessions/{id}/messages",
  editQueued: "PATCH /v1/sessions/{id}/queue/{mid}",
  deleteQueued: "DELETE /v1/sessions/{id}/queue/{mid}",
  promoteQueued: "POST /v1/sessions/{id}/queue/{mid}/promote",
  resolveTicket: "POST /v1/tickets/{id}/resolve",
  rejectTicket: "POST /v1/tickets/{id}/reject",
  ticketDone: "POST /v1/tickets/{id}/done",
  cancelSubagent: "POST /v1/sessions/{id}/subagents/{sid}/cancel",
  listTickets: "GET /v1/tickets",
  searchSkills: "GET /v1/skills",
  listArtifacts: "GET /v1/sessions/{id}/artifacts",
  listVisuals: "GET /v1/sessions/{id}/visuals",
  artifactBlob: "GET /v1/sessions/{id}/artifacts/{path}",
  faviconBlob: "GET /v1/favicons/{origin}",
  takeover: "GET /v1/tickets/{id}/takeover",
  browserPreview: "GET /v1/sessions/{id}/browser/preview",
  browserThumbnail: "GET /v1/sessions/{id}/browser/thumbnails/{thumbnail_id}",
  browserTabs: "GET /v1/browser/tabs",
  openSessionStream: "GET /v1/sessions/{id}/events",
  pollSessionEvents: "GET /v1/sessions/{id}/events/poll",
  sandboxPreview: "GET /v1/sessions/{id}/artifacts/{path}/preview",
  listSkills: "GET /v1/skills",
  getSkill: "GET /v1/skills/{name}",
  createSkill: "POST /v1/skills",
  updateSkill: "PATCH /v1/skills/{name}",
  deleteSkill: "DELETE /v1/skills/{name}",
  listRoutines: "GET /v1/routines",
  createRoutine: "POST /v1/routines",
  updateRoutine: "PATCH /v1/routines/{id}",
  deleteRoutine: "DELETE /v1/routines/{id}",
  runRoutine: "POST /v1/routines/{id}/run",
  routineRuns: "GET /v1/routines/{id}/runs",
  routineSuggestions: "GET /v1/routines/suggestions",
  searchMemory: "GET /v1/memory",
  memoryHealth: "GET /v1/memory/health",
  createMemory: "POST /v1/memory",
  updateMemory: "PATCH /v1/memory/{id}",
  deleteMemory: "DELETE /v1/memory/{id}",
  memoryHistory: "GET /v1/memory/{id}/history",
  settingsConfig: "GET /v1/settings/config",
  updateSettingsConfig: "PUT /v1/settings/config",
  onboarding: "GET /v1/onboarding",
  setOnboarding: "PUT /v1/onboarding",
  externalAgents: "GET /v1/external-agents",
  installAgentSkill: "POST /v1/external-agents/{id}/browser-skill",
  importProfiles: "GET /v1/import/profiles",
  runImport: "POST /v1/import",
  previewImport: "POST /v1/import/preview",
  importJob: "GET /v1/import/jobs/{id}",
  importPasswordsCsv: "POST /v1/import/passwords",
  listImportDelivery: "GET /v1/import/delivery",
  mintImportDeliveryTicket: "POST /v1/import/delivery/{id}/ticket",
  acknowledgeImportDelivery: "POST /v1/import/delivery/{id}/ack",
  vaultStatus: "GET /v1/vault",
  setupVault: "POST /v1/vault/setup",
  unlockVault: "POST /v1/vault/unlock",
  lockVault: "POST /v1/vault/lock",
  createVaultItem: "POST /v1/vault/items",
  deleteVaultItem: "DELETE /v1/vault/items/{id}",
  vaultItems: "GET /v1/vault/items",
  updateVaultItem: "PATCH /v1/vault/items/{id}",
  vaultAudit: "GET /v1/vault/audit",
  connections: "GET /v1/connections",
  updateConnection: "PATCH /v1/connections/{id}",
  checkConnection: "POST /v1/connections/{id}/check",
  reloginConnection: "POST /v1/connections/{id}/relogin",
  createAccountSpace: "POST /v1/connections/spaces",
  revokeConnection: "POST /v1/connections/{id}/revoke",
  dismissConnectionCandidate: "POST /v1/connections/candidates/{id}/dismiss",
  connectionAudit: "GET /v1/connections/audit",
  listDevices: "GET /v1/devices",
  revokeDevice: "DELETE /v1/devices/{id}",
  myDevice: "GET /v1/devices/me",
  requestAccess: "POST /v1/devices/me/access-requests",
  accessRequests: "GET /v1/access-requests",
  approveAccessRequest: "POST /v1/access-requests/{id}/approve",
  denyAccessRequest: "POST /v1/access-requests/{id}/deny",
  exchangePairing: "POST /v1/pairing/exchange",
  createPairing: "POST /v1/pairing",
  remoteStatus: "GET /v1/remote/status",
  setTailscale: "PUT /v1/remote/tailscale",
  setCloudflare: "PUT /v1/remote/cloudflare",
  setRelay: "PUT /v1/remote/relay",
  vapidKey: "GET /v1/push/vapid",
  listPushSubscriptions: "GET /v1/push/subscriptions",
  createPushSubscription: "POST /v1/push/subscriptions",
  deletePushSubscription: "DELETE /v1/push/subscriptions/{id}",
  pushPresence: "POST /v1/push/presence",
  setExternalAgent: "PUT /v1/external-agents/{id}",
  deleteExternalAgent: "DELETE /v1/external-agents/{id}",
  daemonStatus: "GET /v1/daemon",
  restartDaemon: "POST /v1/daemon/restart",
  doctor: "GET /v1/doctor",
  memoryModel: "GET /v1/memory/model",
  downloadMemoryModel: "POST /v1/memory/model/download",
  cancelMemoryModel: "POST /v1/memory/model/cancel",
  policy: "GET /v1/policy",
  setPolicy: "PUT /v1/policy",
  testPolicy: "POST /v1/policy/test",
  validatePolicy: "POST /v1/policy/validate",
  networkPolicy: "GET /v1/policy/network",
  setNetworkPolicy: "PUT /v1/policy/network",
  syncStatus: "GET /v1/sync",
  setupSync: "POST /v1/sync/setup",
  pairSync: "POST /v1/sync/pair",
  unlockSync: "POST /v1/sync/unlock",
  runSync: "POST /v1/sync/run",
  setSyncConfig: "PUT /v1/sync/config",
  pinnedTabs: "GET /v1/pinned-tabs",
  pinTab: "POST /v1/pinned-tabs",
  unpinTab: "DELETE /v1/pinned-tabs/{id}",
};

export interface UiFeature {
  /** Route (`/skills`), or `token`/`shell` for the sign-in screen and the app frame. */
  screen: string;
  feature: string;
  /** Files under `web/src` that make these calls. */
  sources: string[];
  calls: CallName[];
  /** Only for features without any daemon call: why none is needed. */
  clientOnly?: string;
}

export const FEATURES: UiFeature[] = [
  { screen: "token", feature: "Paste a device token", sources: ["components/TokenEntry.tsx"], calls: ["health"] },
  { screen: "/pair", feature: "Pair via QR deep link (`/pair#code`), store the device token", sources: ["pages/PairPage.tsx"], calls: ["exchangePairing"] },
  { screen: "/settings", feature: "Settings dialog: daemon connection status, re-checked every 5 s while open", sources: ["pages/SettingsPage.tsx"], calls: ["health"] },
  { screen: "/settings", feature: "Enable / disable notifications for this device (explicit click; Home Screen instructions on iOS tabs); desktop notifications through the openside extension where Web Push has no push service, which pairs the extension with a one-time code", sources: ["pages/SettingsPage.tsx", "push.ts", "desktopNotify.ts"], calls: ["vapidKey", "createPushSubscription", "listPushSubscriptions", "deletePushSubscription", "createPairing", "revokeDevice"] },
  { screen: "/settings", feature: "Sign out of this browser", sources: ["pages/SettingsPage.tsx"], calls: [], clientOnly: "Removes the stored device token; revoking the device is under Devices." },
  { screen: "/settings", feature: "Entry to Models & accounts (`/models`, a dialog over the current screen like Settings)", sources: ["pages/SettingsPage.tsx", "pages/ManageDialogs.tsx", "pages/manageRoutes.ts"], calls: [], clientOnly: "Navigation only; the screen loads its own data." },
  { screen: "/workspace", feature: "Workspace dialog: section tiles; every section keeps its deep-link route", sources: ["pages/ManageDialogs.tsx", "pages/manageRoutes.ts"], calls: [], clientOnly: "Navigation only; each section loads its own data." },
  { screen: "shell", feature: "Report foreground presence so focused devices get no push", sources: ["push.ts"], calls: ["pushPresence"] },
  { screen: "shell", feature: "Floating collapsed card for another conversation's code, login, choice or phone-approval request, over any screen; no navigation", sources: ["components/CardAttention.tsx", "cards/actions.ts", "cards/media.ts"], calls: ["listTickets", "resolveTicket", "rejectTicket", "faviconBlob"] },
  { screen: "shell", feature: "Appearance: theme mode, light/dark palette presets and primary color", sources: ["components/AppearanceSettings.tsx", "theme.ts"], calls: [], clientOnly: "Stored in this browser only." },
  { screen: "shell", feature: "Responsive (mobile-first) layout", sources: ["styles.css"], calls: [], clientOnly: "CSS only." },
  { screen: "shell", feature: "Authorize single-use native imports for the paired openside extension; acknowledge only successful authenticated delivery", sources: ["import/delivery.ts"], calls: ["listImportDelivery", "mintImportDeliveryTicket", "acknowledgeImportDelivery"] },
  { screen: "shell", feature: "Sidebar people list: the assistant first, then teammates (pinned, then latest message); avatar, live activity or last message, working dots, Needs you badge, unread dot from a per-device seen cursor; hidden teammates folded away; re-synced while visible", sources: ["hooks/useAgents.ts", "components/Sidebar.tsx", "state/seen.ts"], calls: ["listAgents"] },
  { screen: "shell", feature: "Retire a teammate (hand over to a successor or pause their routines) from the thread menu, the profile, Team or a card; Undo on an explicit hire card", sources: ["components/TeamActions.tsx", "components/AgentProfileSheet.tsx"], calls: ["retireAgent", "resolveTicket", "listTickets"] },
  { screen: "shell", feature: "Messages typed while offline, including ones from before conversations belonged to people, are delivered when the daemon is back", sources: ["pwa/outbox.ts"], calls: ["createSession", "submitMessage"] },
  { screen: "/agents/:id", feature: "One continuous thread with each person: header opens the profile, menu hides or shows them in the sidebar and retires teammates; the assistant greets with starters when the thread is empty", sources: ["pages/AgentPage.tsx", "pages/ThreadView.tsx"], calls: ["updateAgent"] },
  { screen: "/agents/:id", feature: "Profile: name, initials, colour, role, voice, responsibilities, boundaries, model, account pool, permission ceiling, channel identity, skills, owned routines; only changed fields are saved", sources: ["components/AgentProfileSheet.tsx", "components/ModelPicker.tsx"], calls: ["updateAgent", "modelCatalog", "listPools", "channelAccounts", "listSkills", "listRoutines"] },
  { screen: "/history", feature: "Earlier conversations from before the team: search, Archived tab, row menu with confirmed deletion; retired teammates' threads, read only with Restore", sources: ["pages/HistoryPage.tsx", "hooks/useSessionList.ts", "components/SessionMenu.tsx", "pages/AgentPage.tsx"], calls: ["listSessions", "updateSession", "deleteSession", "restoreAgent"] },
  { screen: "/team", feature: "Team: everyone with role, owned work and state; add a teammate directly; retire or restore", sources: ["pages/TeamPage.tsx"], calls: ["createAgent", "restoreAgent"] },
  { screen: "/team", feature: "Suggestions you turned off: teammate topics answered with Never, and Allow again", sources: ["pages/TeamPage.tsx"], calls: ["suppressedProposals", "clearSuppressedProposal"] },
  { screen: "/", feature: "Opens the assistant's thread", sources: ["App.tsx"], calls: [], clientOnly: "Redirect only; the thread loads its own data." },
  { screen: "/agents/:id", feature: "Model picker: every model grouped by provider, unavailable ones disabled with the reason, the daemon default, a custom `provider:model` entry; falls back to the configured default and fallbacks if the catalog cannot be read", sources: ["components/ModelPicker.tsx"], calls: ["modelCatalog", "settings"] },
  { screen: "/sessions/:id", feature: "Live event stream; screen rebuilt from the event log on reload", sources: ["hooks/useSessionView.ts"], calls: ["openSessionStream"] },
  { screen: "/sessions/:id", feature: "Long-poll fallback by sequence when a proxy or tunnel buffers or stalls the live stream; same cursor, no duplicates or gaps", sources: ["api/sse.ts"], calls: ["pollSessionEvents"] },
  { screen: "/sessions/:id", feature: "Session menu (rename, pin, archive, copy link, confirmed Delete conversation for ordinary threads only); stop from the composer; no Delete or Clear for agent home threads", sources: ["components/SessionMenu.tsx", "pages/ThreadView.tsx"], calls: ["updateSession", "deleteSession", "stopSession"] },
  { screen: "/sessions/:id", feature: "Send, or queue while a run is active; image attachments; focusing the box warms the agent and browser", sources: ["state/outgoing.ts", "components/Composer.tsx"], calls: ["submitMessage", "prewarmSession"] },
  { screen: "/sessions/:id", feature: "`/` skill mention chips", sources: ["components/Composer.tsx"], calls: ["searchSkills"] },
  { screen: "/sessions/:id", feature: "Queued messages: edit, cancel, steer instead / send now", sources: ["components/QueueList.tsx"], calls: ["editQueued", "deleteQueued", "promoteQueued"] },
  { screen: "/sessions/:id", feature: "Conversation cards: one collapsed bubble that expands for every request (approval, question, secret, code, login, choice, payment, card, form, hire, takeover), read-only result cards (products, tables, places, orders), and activity and hand-off rows; secrets are never echoed", sources: ["components/ConversationCard.tsx", "cards/render.ts", "cards/actions.ts", "cards/blocks.ts", "cards/fields.ts", "cards/media.ts"], calls: ["resolveTicket", "rejectTicket", "ticketDone", "faviconBlob", "artifactBlob"] },
  { screen: "/sessions/:id", feature: "`?ticket=` from a code, login or choice push opens the request as a full-screen card sheet; the inline card stays inert meanwhile", sources: ["components/CardSheet.tsx", "pages/ThreadView.tsx", "cards/actions.ts"], calls: ["resolveTicket", "rejectTicket"] },
  { screen: "/sessions/:id", feature: "Takeover remote screen and PiP preview", sources: ["components/TakeoverView.tsx"], calls: ["takeover", "ticketDone", "rejectTicket"] },
  { screen: "/sessions/:id", feature: "Live browser card of an agent that browses itself, docked at the top of the conversation: read-only screencast, agent status, Action needed, click ripple, minimize, stop", sources: ["components/BrowserPreview.tsx", "hooks/useBrowserPreview.ts"], calls: ["browserPreview"] },
  { screen: "/sessions/:id", feature: "Background Tasks: one unit per Task under the message that started it, its current step with earlier steps faint beneath; a Task that browses shows its tab's live screencast or authenticated last picture while it works, with ✕ to stop it after a confirmation", sources: ["components/TaskUnit.tsx", "hooks/useBrowserPreview.ts"], calls: ["browserPreview", "browserThumbnail", "cancelSubagent"] },
  { screen: "/sessions/:id", feature: "Browser sheet: this conversation's agent tabs with titles and URLs, live preview and zoom", sources: ["components/BrowserSheet.tsx"], calls: ["browserTabs"] },
  { screen: "/sessions/:id", feature: "Agents pill in a teammate conversation's composer: a read-only sheet of delegated subagents with their steps", sources: ["components/SubagentPanel.tsx"], calls: [], clientOnly: "Drawn from the conversation's event stream; it needs no call of its own." },
  { screen: "/sessions/:id", feature: "Files pill in a teammate conversation's composer opens a sheet: list, sandboxed preview, download (the chief conversation has none)", sources: ["hooks/useSessionFiles.ts", "components/FilesTray.tsx", "components/VisualFrame.tsx"], calls: ["listArtifacts", "artifactBlob", "sandboxPreview"] },
  { screen: "/sessions/:id", feature: "Documents attached to an agent message, at the top of its bubble: inline ones read in place with Show more; panel ones open beside the thread (a sheet on a phone) with Copy and Close", sources: ["components/MessageDocument.tsx", "state/session.ts"], calls: ["artifactBlob"] },
  { screen: "/sessions/:id", feature: "Visual blocks: inert images sized to their content (SVG, or HTML laid out in an SVG foreignObject) and their source", sources: ["hooks/useSessionFiles.ts", "components/VisualFrame.tsx", "components/visualImage.ts"], calls: ["listVisuals"] },
  { screen: "/welcome", feature: "First-run guide: opens once on a new daemon, can be skipped, and is reopened from Settings; whether it was seen is kept on the daemon", sources: ["onboarding/OnboardingGate.tsx", "onboarding/OnboardingPage.tsx"], calls: ["onboarding", "setOnboarding"] },
  { screen: "/welcome", feature: "Guide step: connect a model (subscription login or API key) before the first conversation; skippable", sources: ["onboarding/ModelsStep.tsx", "models/ModelsPanel.tsx", "models/useModelSettings.ts"], calls: ["modelCatalog"] },
  { screen: "/welcome", feature: "Guide step: detect the claude, codex and omp agents and install the openside browser skill for them", sources: ["onboarding/AgentsStep.tsx", "pages/AgentsPage.tsx"], calls: ["externalAgents", "installAgentSkill"] },
  { screen: "/welcome", feature: "Guide step: import all local browser profiles with per-kind preview, encrypted Vault password migration, progress and a restore-tabs list", sources: ["onboarding/ImportStep.tsx"], calls: ["importProfiles", "previewImport", "runImport", "importJob", "importPasswordsCsv"] },
  { screen: "/welcome", feature: "Guide step: set up or unlock the Vault inside the import step when passwords need it, without leaving the guide", sources: ["onboarding/VaultGate.tsx", "onboarding/ImportStep.tsx"], calls: ["vaultStatus", "setupVault", "unlockVault"] },
  { screen: "/welcome", feature: "Guide step: connect a phone with the same QR flow as Remote access, the code shown at once (a notice, not a command, when this browser is not on the computer); the step notices the new device", sources: ["onboarding/PairStep.tsx", "remote/ConnectPhone.tsx", "remote/transports.ts"], calls: ["remoteStatus", "createPairing", "listDevices", "setTailscale", "setCloudflare", "setRelay"] },
  { screen: "/vault", feature: "A paired device without the admin scope asks for it on the screen and sees when it is approved; no command to run", sources: ["pages/ScopeNotice.tsx"], calls: ["myDevice", "requestAccess"] },
  { screen: "/", feature: "On the computer running openside, a floating card shows a device asking for access with Allow and Decline", sources: ["access/AccessRequestDock.tsx"], calls: ["accessRequests", "approveAccessRequest", "denyAccessRequest"] },
  { screen: "/devices", feature: "Create a one-time link for an agent or CLI from the screen, with the terminal command only as a hint", sources: ["pages/AgentLink.tsx"], calls: ["createPairing"] },
  { screen: "/agents", feature: "Coding agents: which of claude, codex and omp openside found, install its browser skill, add another agent by its skills folder, change or forget that folder", sources: ["pages/AgentsPage.tsx", "pages/AgentFolderDialog.tsx"], calls: ["externalAgents", "installAgentSkill", "setExternalAgent", "deleteExternalAgent"] },
  { screen: "/sync", feature: "Sync: choose a shared folder or WebDAV, what is synced, start or join an account, unlock with the passphrase, sync now with counts", sources: ["pages/SyncPage.tsx"], calls: ["syncStatus", "setSyncConfig", "setupSync", "pairSync", "unlockSync", "runSync"] },
  { screen: "/sync", feature: "Pinned tabs that sync between devices: list, add by address, remove", sources: ["pages/SyncPage.tsx"], calls: ["pinnedTabs", "pinTab", "unpinTab"] },
  { screen: "/policy", feature: "Policy: default mode, origin allow and deny lists and Allow/Ask/Deny rules, checked row by row before saving", sources: ["pages/PolicyPage.tsx"], calls: ["policy", "validatePolicy", "setPolicy"] },
  { screen: "/policy", feature: "Network allow-list for the daemon and its agents, restart to apply", sources: ["pages/PolicyPage.tsx", "pages/RestartNotice.tsx"], calls: ["networkPolicy", "validatePolicy", "setNetworkPolicy", "restartDaemon", "daemonStatus"] },
  { screen: "/policy", feature: "Try an action against the live policy and see the decision and why", sources: ["pages/PolicyPage.tsx"], calls: ["testPolicy"] },
  { screen: "/diagnostics", feature: "Diagnostics: the daemon's checks grouped by area, each with a Fix button that downloads, restarts or opens the screen that fixes it", sources: ["pages/DiagnosticsPage.tsx", "pages/RestartNotice.tsx"], calls: ["doctor", "downloadMemoryModel", "restartDaemon", "daemonStatus"] },
  { screen: "/skills", feature: "Skill list with rejected folders", sources: ["pages/SkillsPage.tsx"], calls: ["listSkills"] },
  { screen: "/skills", feature: "Create a skill", sources: ["pages/SkillsPage.tsx"], calls: ["createSkill"] },
  { screen: "/skills", feature: "Edit description and instructions", sources: ["pages/SkillsPage.tsx"], calls: ["getSkill", "updateSkill"] },
  { screen: "/skills", feature: "Enable / disable, delete", sources: ["pages/SkillsPage.tsx"], calls: ["updateSkill", "deleteSkill"] },
  { screen: "/routines", feature: "Routine list with schedule and next run", sources: ["pages/RoutinesPage.tsx"], calls: ["listRoutines"] },
  { screen: "/routines", feature: "Create a cron or interval routine", sources: ["pages/RoutinesPage.tsx"], calls: ["createRoutine"] },
  { screen: "/routines", feature: "Run now, pause / resume, delete", sources: ["pages/RoutinesPage.tsx"], calls: ["runRoutine", "updateRoutine", "deleteRoutine"] },
  { screen: "/routines", feature: "Run history", sources: ["pages/RoutinesPage.tsx"], calls: ["routineRuns"] },
  { screen: "/routines", feature: "Suggested routines from repeated requests", sources: ["pages/RoutinesPage.tsx"], calls: ["routineSuggestions", "createRoutine"] },
  { screen: "/memory", feature: "Memory count and search mode", sources: ["pages/MemoryPage.tsx"], calls: ["memoryHealth"] },
  { screen: "/memory", feature: "Search memory, filter by kind", sources: ["pages/MemoryPage.tsx"], calls: ["searchMemory"] },
  { screen: "/memory", feature: "Create, edit (content, tags, retention), delete", sources: ["pages/MemoryPage.tsx"], calls: ["createMemory", "updateMemory", "deleteMemory"] },
  { screen: "/memory", feature: "Change history", sources: ["pages/MemoryPage.tsx"], calls: ["memoryHistory"] },
  { screen: "/memory", feature: "Search model card: download the local embedding model with byte progress and Cancel, see why a download failed, restart to start vector search", sources: ["pages/MemoryModelCard.tsx", "pages/RestartNotice.tsx"], calls: ["memoryModel", "downloadMemoryModel", "cancelMemoryModel", "restartDaemon", "daemonStatus"] },
  { screen: "/models", feature: "Models & accounts: providers with their models, availability and the reason when unavailable", sources: ["models/useModelSettings.ts", "models/ProviderCard.tsx"], calls: ["modelCatalog"] },
  { screen: "/models", feature: "Fast and reasoning models, set once, plus ordered fallbacks", sources: ["models/ModelsPanel.tsx"], calls: ["setModels"] },
  { screen: "/models", feature: "API keys per provider, any number: add with a name, rename, replace, keep-in-reserve percentage, on/off, remove; only the last four characters are ever shown; hidden from limited devices", sources: ["models/useModelSettings.ts", "models/ModelsPanel.tsx", "models/ProviderCard.tsx", "models/AccountDialogs.tsx"], calls: ["listApiKeys", "addApiKey", "replaceApiKey", "updateApiKey", "deleteApiKey"] },
  { screen: "/models", feature: "Claude Code and Codex CLI subscription accounts, any number: add with a name, see the exact sign-in command and wait for the sign-in to land, rename, reserve, on/off, remove", sources: ["models/useModelSettings.ts", "models/ProviderCard.tsx", "models/ModelsPanel.tsx", "models/AccountDialogs.tsx"], calls: ["listSubscriptions", "addSubscriptionAccount", "subscriptionAccount", "updateSubscriptionAccount", "deleteSubscriptionAccount"] },
  { screen: "/models", feature: "Rotation order per provider: which accounts serve requests, reorder, add and remove; new accounts join only when chosen", sources: ["models/ProviderCard.tsx", "models/ModelsPanel.tsx", "models/useModelSettings.ts"], calls: ["listPools", "setPool"] },
  { screen: "/", feature: "Conversation menu: prefer one account in the provider's rotation for the next messages, or return to automatic", sources: ["components/SessionMenu.tsx", "models/SessionAccountDialog.tsx"], calls: ["setSessionAccount"] },
  { screen: "/models", feature: "Broker accounts with remaining usage per window and rate-limit blocks, and each credential source's availability", sources: ["models/ModelsPanel.tsx", "models/ProviderCard.tsx", "models/useModelSettings.ts"], calls: ["accounts"] },
  { screen: "/vault", feature: "Vault status: set up, unlock (passphrase or OS keyring) and lock", sources: ["pages/VaultPage.tsx"], calls: ["vaultStatus", "setupVault", "unlockVault", "lockVault"] },
  { screen: "/vault", feature: "Add a login or an authenticator; delete an item", sources: ["pages/VaultPage.tsx"], calls: ["createVaultItem", "deleteVaultItem"] },
  { screen: "/vault", feature: "Vault items (metadata only, never values)", sources: ["pages/VaultPage.tsx"], calls: ["vaultItems"] },
  { screen: "/vault", feature: "Per-item use policy", sources: ["pages/VaultPage.tsx"], calls: ["updateVaultItem"] },
  { screen: "/vault", feature: "Audit log with item / session / time filters", sources: ["pages/VaultPage.tsx"], calls: ["vaultAudit"] },
  { screen: "/connections", feature: "Sites your agents are signed in to: favicon, site, masked account, status (Active, Sign in again, Expired, Not checked), last use and source; Check now", sources: ["pages/ConnectionsPage.tsx"], calls: ["connections", "checkConnection"] },
  { screen: "/connections", feature: "Sign in again: finishes silently from the Vault or jumps to the sign-in ticket in the conversation", sources: ["pages/ConnectionsPage.tsx"], calls: ["reloginConnection"] },
  { screen: "/connections", feature: "Sign out of a site in the agent browser or forget a connection, each after a confirmation; optionally delete the linked Vault login", sources: ["pages/ConnectionsPage.tsx", "pages/ConnectionDialogs.tsx"], calls: ["revokeConnection"] },
  { screen: "/connections", feature: "Per-agent access (allowed, ask first, blocked) for each connection, and the same choice per agent in a teammate's profile", sources: ["pages/ConnectionDialogs.tsx", "components/AgentConnections.tsx"], calls: ["updateConnection", "connections"] },
  { screen: "/connections", feature: "Possibly signed in: sites the browser holds cookies for but nobody verified, collapsed; Check or Dismiss", sources: ["pages/ConnectionsPage.tsx"], calls: ["checkConnection", "dismissConnectionCandidate"] },
  { screen: "/connections", feature: "Add another account on a site as a separate browser profile (what it costs is shown first), then sign in through the usual ticket", sources: ["pages/ConnectionDialogs.tsx"], calls: ["createAccountSpace"] },
  { screen: "/connections", feature: "Activity: every use and change of a connection, newest first, filtered by connection, agent and action", sources: ["pages/ConnectionsPage.tsx"], calls: ["connectionAudit"] },
  { screen: "/channels", feature: "Entry to Channels from Settings (`/channels`, a dialog over the current screen)", sources: ["pages/SettingsPage.tsx", "pages/ManageDialogs.tsx", "pages/manageRoutes.ts"], calls: [], clientOnly: "Navigation only; the screen loads its own data." },
  { screen: "/channels", feature: "Telegram, Slack and Discord accounts, any number each: label, status, the teammate that answers (name and avatar), the callback address with Copy for Slack and Discord, and a hint to move legacy config.toml accounts into the vault; read-only on devices without admin", sources: ["channels/ChannelsPanel.tsx", "channels/WebhookUrl.tsx"], calls: ["channelAccounts"] },
  { screen: "/channels", feature: "Add or edit an account in a dialog (name, id, teammate, allow-lists, write-only credentials read from the form at submit and cleared), reconnect or replace credentials, turn one account on or off", sources: ["channels/AccountDialog.tsx", "channels/ChannelsPanel.tsx"], calls: ["addChannelAccount", "updateChannelAccount"] },
  { screen: "/channels", feature: "Test connection: a read-only identity check of one account, with a sanitized result", sources: ["channels/AccountDialog.tsx", "channels/ChannelsPanel.tsx"], calls: ["testChannelAccount"] },
  { screen: "/channels", feature: "Remove one account after a confirmation; the others stay connected", sources: ["channels/ChannelsPanel.tsx"], calls: ["deleteChannelAccount"] },
  { screen: "/channels", feature: "Restart notice with a Restart daemon button when an account needs the daemon to restart", sources: ["channels/ChannelsPanel.tsx", "pages/RestartNotice.tsx"], calls: ["restartDaemon", "daemonStatus"] },
  { screen: "/remote", feature: "Remote access: state of Tailscale, Cloudflare Tunnel and a self-hosted relay (off, starting, on with its address, error with a fix hint), a copy-address button, restart notice, and where to go when this browser is not on the computer", sources: ["remote/RemoteAccess.tsx", "remote/LocalOnlyNotice.tsx"], calls: ["remoteStatus"] },
  { screen: "/remote", feature: "Restart notice with a Restart daemon button once a connection change is saved", sources: ["remote/RemoteAccess.tsx", "pages/RestartNotice.tsx"], calls: ["restartDaemon", "daemonStatus"] },
  { screen: "/remote", feature: "Turn a connection on or off and edit its settings (Cloudflare quick or named tunnel, relay address and token file)", sources: ["remote/RemoteAccess.tsx"], calls: ["setTailscale", "setCloudflare", "setRelay"] },
  { screen: "/remote", feature: "Connect your phone: the first section of Remote access shows a locally drawn QR code at once, made for the best connection that is on, with a status line (waiting, connected as the phone's name), a copyable link, a 3-minute countdown with a new code; Change connection and Phone name and permissions are secondary controls; a connection that is down shows its state instead of a code, with one click to turn the default connection on", sources: ["remote/ConnectPhone.tsx", "remote/transports.ts"], calls: ["createPairing", "listDevices", "setTailscale", "setCloudflare", "setRelay"] },
  { screen: "/remote", feature: "Take the desktop launcher's local-access proof from the URL fragment (stored for the tab only, fragment removed at once) or pasted by hand", sources: ["remote/LocalOnlyNotice.tsx", "localAccess.ts"], calls: [], clientOnly: "Local-access proof is kept in sessionStorage and sent as a header on the remote-access calls; it needs no call of its own." },
  { screen: "/devices", feature: "Paired devices, revoke after confirmation", sources: ["pages/DevicesPage.tsx"], calls: ["listDevices", "revokeDevice"] },
  { screen: "/config", feature: "Every configuration key, rendered from the daemon's schema; local-only keys read-only for paired devices", sources: ["pages/ConfigPage.tsx"], calls: ["settingsConfig", "updateSettingsConfig"] },
  { screen: "/config", feature: "Edit the whole document as JSON", sources: ["pages/ConfigPage.tsx"], calls: ["updateSettingsConfig"] },
  { screen: "/config", feature: "Explains which keys a paired device cannot change and why; restart notice with Restart daemon after saving", sources: ["pages/ConfigPage.tsx", "pages/RestartNotice.tsx"], calls: ["restartDaemon", "daemonStatus"] },
  // The browser extension's side panel imports this same client (G-5: one public API for every client).
  { screen: "extension", feature: "Side panel: pending approval cards on reopen; code requests from any conversation as the shared code form; badge and notification", sources: ["../../extension/src/panel.ts", "../../extension/src/codes.ts", "cards/actions.ts"], calls: ["listTickets", "resolveTicket"] },
];

/** Public endpoints with no web UI feature, and why. */
export const UNSURFACED: Partial<Record<Endpoint, string>> = {
  "POST /v1/devices/pair": "Local socket only: token minting for `openside token create`.",
  "GET /v1/remote/local-access": "Local socket only: `openside-browser` and `openside remote local-access` read the per-boot proof and hand it to the page in the launch link fragment; the page never requests it.",
  "GET /v1/import/jobs/{id}/events": "The guide polls GET /v1/import/jobs/{id}; the SSE stream is available to other local clients.",
  "POST /v1/otp": "Machine endpoint for an SMS forwarder holding the `otp_ingest` scope.",
  "POST /v1/sessions/{id}/repl": "Machine endpoint (`browse` scope) for sandboxed tools, MCP servers and extensions.",
  "GET /v1/tools": "Machine endpoint: the catalog `openside mcp` and `openside browser tools` build their direct browser tools from.",
  "POST /v1/sessions/{id}/tools/{name}": "Machine endpoint (`browse` scope) for the CLI, MCP servers and scripts driving a session's browser and human-step tools directly; the tickets it opens appear in the web UI as usual.",
  "DELETE /v1/connections/spaces/{space_id}": "Removing an account from its card revokes the connection, which deletes the separate profile; the profile id is never shown.",
  "GET /v1/sessions/{id}": "The session screen rebuilds the session from its event stream.",
  "GET /v1/connections/{id}": "The list carries every field of a connection, so cards never read one on its own.",
  "GET /v1/agents/{id}/handoffs": "Hand-offs are opened by agents and by `openside` clients; a thread shows them as hand-off cards from its own events, never as a list.",
  "POST /v1/agents/{id}/handoffs": "Hand-offs are opened by agents and by `openside` clients; people ask a teammate for work by writing in its thread.",
  "POST /v1/agents/{id}/transfer": "Moving routines and memory between teammates happens through Retire (POST /v1/agents/{id}/retire, hand over); a separate transfer stays an API action.",
  "GET /v1/extension/import/{ticket}": "Native delivery for the browser extension: it fetches the one-use ticket with its own device token and verifies the signature before writing; the payload never passes through a web page.",
  "GET /v1/agents/{id}": "The sidebar, threads and profiles read the roster from GET /v1/agents, which carries every field the UI shows.",
  "POST /v1/browser/prewarm": "Conversations belong to people now: focusing a thread's box warms that session (POST /v1/sessions/{id}/prewarm); browser-only warm-up stays for other clients.",
  "POST /v1/sessions/{id}/queue/{mid}/move": "Not offered in the web UI: the queue offers edit, delete and send now; reordering stays an API action.",
  "GET /v1/sessions/{id}/subagents": "The subagent panel is built from `subagent.*` events.",
  "POST /v1/sessions/{id}/subagents/{sid}/steer": "People steer work by talking to the agent that started it; Task units and the agents sheet are read-only. Steering stays an API action for other clients.",
  "GET /v1/tickets/{id}": "Ticket cards come from `ticket.*` events of the open session.",
  "GET /v1/memory/{id}": "Search hits already carry the full memory.",
  "GET /v1/routines/{id}": "The routine list already carries every field.",
  "POST /v1/hooks/{kind}/{account_id}": "Public ingress that Slack and Discord post to, signed by the platform and never called by the UI; each account's row shows this address with a copy button.",
};

interface OpenApiOperation {
  security?: unknown[];
  "x-required-capability"?: string;
}

export interface OpenApiDocument {
  paths: Record<string, Record<string, OpenApiOperation>>;
}

const VERBS = ["get", "put", "post", "delete", "patch"];

export function openApiEndpoints(document: OpenApiDocument): string[] {
  return Object.entries(document.paths).flatMap(([path, item]) =>
    Object.keys(item)
      .filter((verb) => VERBS.includes(verb))
      .map((verb) => `${verb.toUpperCase()} ${path}`),
  );
}

function scope(document: OpenApiDocument, endpoint: string): string {
  const [verb = "", path = ""] = endpoint.split(" ");
  const operation = document.paths[path]?.[verb.toLowerCase()];
  if (!operation) return "missing";
  return operation.security?.length === 0 ? "public" : (operation["x-required-capability"] ?? "");
}

const cell = (text: string) => text.replace(/\|/g, "\\|");

/** The committed `docs/specs/ui-api-map.md`. */
export function renderUiApiMap(document: OpenApiDocument): string {
  const used = new Set(FEATURES.flatMap((feature) => feature.calls.map((call) => CALLS[call])));
  const all = openApiEndpoints(document);
  const lines = [
    "# Web UI ↔ API map",
    "",
    "<!-- Generated by `pnpm -C web gen:ui-map` from `web/src/api/uiFeatures.ts`; do not edit. -->",
    "",
    "Every web UI feature and the public daemon endpoints it calls (PP-16, E-4, G-5). `web/src/api/uiApiMap.test.ts` fails when a feature has no endpoint, when web code opens a network connection outside `web/src/api/`, or when an endpoint is missing from `docs/specs/openapi.json`.",
    "",
    `Coverage: ${FEATURES.length} features, ${used.size} of ${all.length} endpoints used by the UI, ${Object.keys(UNSURFACED).length} listed below with a reason.`,
    "",
    "## Features",
    "",
    "| Screen | Feature | Endpoints (scope) | Calls |",
    "| --- | --- | --- | --- |",
    ...FEATURES.map((feature) => {
      const endpoints = feature.calls.length
        ? [...new Set(feature.calls.map((call) => CALLS[call]))]
            .map((endpoint) => `\`${endpoint}\` (${scope(document, endpoint)})`)
            .join("<br>")
        : `— (${feature.clientOnly ?? ""})`;
      const calls = feature.calls.map((call) => `\`${call}\``).join(", ");
      return `| \`${feature.screen}\` | ${cell(feature.feature)} | ${endpoints} | ${calls} |`;
    }),
    "",
    "## Endpoints without a UI feature",
    "",
    "| Endpoint (scope) | Reason |",
    "| --- | --- |",
    ...all
      .filter((endpoint) => !used.has(endpoint as Endpoint))
      .map((endpoint) => `| \`${endpoint}\` (${scope(document, endpoint)}) | ${cell(UNSURFACED[endpoint as Endpoint] ?? "**missing reason**")} |`),
    "",
  ];
  return lines.join("\n");
}
