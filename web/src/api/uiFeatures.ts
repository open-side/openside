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
export type CallName = Exclude<keyof Client, Helper> | "openSessionStream";

export const CALLS: Record<CallName, Endpoint> = {
  health: "GET /v1/health",
  accounts: "GET /v1/accounts",
  settings: "GET /v1/settings",
  listSessions: "GET /v1/sessions",
  createSession: "POST /v1/sessions",
  updateSession: "PATCH /v1/sessions/{id}",
  stopSession: "POST /v1/sessions/{id}/stop",
  prewarmSession: "POST /v1/sessions/{id}/prewarm",
  prewarmBrowser: "POST /v1/browser/prewarm",
  submitMessage: "POST /v1/sessions/{id}/messages",
  editQueued: "PATCH /v1/sessions/{id}/queue/{mid}",
  deleteQueued: "DELETE /v1/sessions/{id}/queue/{mid}",
  promoteQueued: "POST /v1/sessions/{id}/queue/{mid}/promote",
  resolveTicket: "POST /v1/tickets/{id}/resolve",
  rejectTicket: "POST /v1/tickets/{id}/reject",
  ticketDone: "POST /v1/tickets/{id}/done",
  cancelSubagent: "POST /v1/sessions/{id}/subagents/{sid}/cancel",
  steerSubagent: "POST /v1/sessions/{id}/subagents/{sid}/steer",
  listTickets: "GET /v1/tickets",
  searchSkills: "GET /v1/skills",
  listArtifacts: "GET /v1/sessions/{id}/artifacts",
  listVisuals: "GET /v1/sessions/{id}/visuals",
  artifactBlob: "GET /v1/sessions/{id}/artifacts/{path}",
  takeover: "GET /v1/tickets/{id}/takeover",
  browserPreview: "GET /v1/sessions/{id}/browser/preview",
  browserThumbnail: "GET /v1/sessions/{id}/browser/thumbnails/{thumbnail_id}",
  browserTabs: "GET /v1/browser/tabs",
  openSessionStream: "GET /v1/sessions/{id}/events",
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
  vaultStatus: "GET /v1/vault",
  setupVault: "POST /v1/vault/setup",
  unlockVault: "POST /v1/vault/unlock",
  lockVault: "POST /v1/vault/lock",
  createVaultItem: "POST /v1/vault/items",
  deleteVaultItem: "DELETE /v1/vault/items/{id}",
  vaultItems: "GET /v1/vault/items",
  updateVaultItem: "PATCH /v1/vault/items/{id}",
  vaultAudit: "GET /v1/vault/audit",
  listDevices: "GET /v1/devices",
  revokeDevice: "DELETE /v1/devices/{id}",
  exchangePairing: "POST /v1/pairing/exchange",
  vapidKey: "GET /v1/push/vapid",
  listPushSubscriptions: "GET /v1/push/subscriptions",
  createPushSubscription: "POST /v1/push/subscriptions",
  deletePushSubscription: "DELETE /v1/push/subscriptions/{id}",
  pushPresence: "POST /v1/push/presence",
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
  { screen: "/settings", feature: "Enable / disable push notifications for this device (explicit click; Home Screen instructions on iOS tabs)", sources: ["pages/SettingsPage.tsx", "push.ts"], calls: ["vapidKey", "createPushSubscription", "listPushSubscriptions", "deletePushSubscription"] },
  { screen: "/settings", feature: "Sign out of this browser", sources: ["pages/SettingsPage.tsx"], calls: [], clientOnly: "Removes the stored device token; revoking the device is under Devices." },
  { screen: "/workspace", feature: "Workspace dialog: section tiles; every section keeps its deep-link route", sources: ["pages/ManageDialogs.tsx", "pages/manageRoutes.ts"], calls: [], clientOnly: "Navigation only; each section loads its own data." },
  { screen: "shell", feature: "Report foreground presence so focused devices get no push", sources: ["push.ts"], calls: ["pushPresence"] },
  { screen: "shell", feature: "Floating code card for another conversation's OTP request, over any screen; no navigation", sources: ["components/OtpDock.tsx", "otp/otpForm.ts"], calls: ["listTickets", "resolveTicket", "rejectTicket"] },
  { screen: "shell", feature: "Appearance: theme mode, light/dark palette presets and primary color", sources: ["components/AppearanceSettings.tsx", "theme.ts"], calls: [], clientOnly: "Stored in this browser only." },
  { screen: "shell", feature: "Responsive (mobile-first) layout", sources: ["styles.css"], calls: [], clientOnly: "CSS only." },
  { screen: "shell", feature: "Sidebar conversation list (pinned group first), re-synced while visible", sources: ["hooks/useSessionList.ts"], calls: ["listSessions"] },
  { screen: "shell", feature: "Search conversations", sources: ["hooks/useSessionList.ts"], calls: ["listSessions"] },
  { screen: "shell", feature: "Row menu: pin / unpin, rename, archive / unarchive; Archived tab", sources: ["components/SessionMenu.tsx", "hooks/useSessionList.ts"], calls: ["updateSession", "listSessions"] },
  { screen: "/", feature: "New chat: the first message creates the conversation, with model choice and an opt-in isolated browser space", sources: ["components/Composer.tsx", "components/ModelPicker.tsx", "pages/NewChatPage.tsx"], calls: ["createSession", "submitMessage", "settings", "prewarmBrowser"] },
  { screen: "/sessions/:id", feature: "Live event stream; screen rebuilt from the event log on reload", sources: ["hooks/useSessionView.ts"], calls: ["openSessionStream"] },
  { screen: "/sessions/:id", feature: "Session menu (rename, pin, archive, copy link); stop from the composer", sources: ["components/SessionMenu.tsx", "pages/SessionPage.tsx"], calls: ["updateSession", "stopSession"] },
  { screen: "/sessions/:id", feature: "Send, or queue while a run is active; image attachments; focusing the box warms the agent and browser", sources: ["components/Composer.tsx"], calls: ["submitMessage", "prewarmSession"] },
  { screen: "/sessions/:id", feature: "`/` skill mention chips", sources: ["components/Composer.tsx"], calls: ["searchSkills"] },
  { screen: "/sessions/:id", feature: "Queued messages: edit, cancel, steer instead / send now", sources: ["components/QueueList.tsx"], calls: ["editQueued", "deleteQueued", "promoteQueued"] },
  { screen: "/sessions/:id", feature: "Approval / info / secret_input / device-approval cards", sources: ["components/TicketCard.tsx"], calls: ["resolveTicket", "rejectTicket", "ticketDone"] },
  { screen: "/sessions/:id", feature: "Inline code form (OTP) with optional authenticator save; `?ticket=` from a code push opens the full-screen code sheet", sources: ["otp/otpForm.ts", "components/OtpSheet.tsx", "pages/SessionPage.tsx"], calls: ["resolveTicket", "rejectTicket"] },
  { screen: "/sessions/:id", feature: "Secure login card (fields from the page, saved accounts, alternative sign-ins, save to vault) and login-step choice card; `?ticket=` opens them as a sheet", sources: ["otp/loginForm.ts", "otp/authCard.ts", "components/AuthForm.tsx", "components/OtpSheet.tsx"], calls: ["resolveTicket", "rejectTicket"] },
  { screen: "/sessions/:id", feature: "Takeover remote screen and PiP preview", sources: ["components/TakeoverView.tsx"], calls: ["takeover", "ticketDone", "rejectTicket"] },
  { screen: "/sessions/:id", feature: "Live browser card docked at the top of the conversation: read-only screencast, agent status, Action needed, click ripple, minimize, cancel task; background Tasks narrate it from browser.state, with authenticated thumbnails and a strip to pick one and cancel per Task", sources: ["components/BrowserPreview.tsx"], calls: ["browserPreview", "browserThumbnail", "cancelSubagent"] },
  { screen: "/sessions/:id", feature: "Browser sheet: this conversation's agent tabs with titles and URLs, live preview and zoom", sources: ["components/BrowserSheet.tsx"], calls: ["browserTabs"] },
  { screen: "/sessions/:id", feature: "Subagent steer / cancel", sources: ["components/SubagentPanel.tsx"], calls: ["steerSubagent", "cancelSubagent"] },
  { screen: "/sessions/:id", feature: "Files panel: list, sandboxed preview, download", sources: ["hooks/useSessionFiles.ts", "components/ArtifactsPanel.tsx", "components/VisualFrame.tsx"], calls: ["listArtifacts", "artifactBlob", "sandboxPreview"] },
  { screen: "/sessions/:id", feature: "Visual blocks: sandboxed preview and source", sources: ["hooks/useSessionFiles.ts", "components/VisualFrame.tsx"], calls: ["listVisuals", "sandboxPreview"] },
  { screen: "/welcome", feature: "First-run guide: opens once on a new daemon, can be skipped, and is reopened from Settings; whether it was seen is kept on the daemon", sources: ["onboarding/OnboardingGate.tsx", "onboarding/OnboardingPage.tsx"], calls: ["onboarding", "setOnboarding"] },
  { screen: "/welcome", feature: "Guide step: detect the claude, codex and omp agents and install the openside browser skill for them", sources: ["onboarding/AgentsStep.tsx"], calls: ["externalAgents", "installAgentSkill"] },
  { screen: "/welcome", feature: "Guide step: import Chrome, Chromium or Firefox history, bookmarks and cookies, with a dry-run preview of the counts before anything is written", sources: ["onboarding/ImportStep.tsx"], calls: ["importProfiles", "runImport"] },
  { screen: "/welcome", feature: "Guide step: vault status with a link to the Vault setup", sources: ["onboarding/VaultStep.tsx"], calls: ["vaultStatus"] },
  { screen: "/welcome", feature: "Guide step: pair a phone with `openside pair`; the step notices the new device", sources: ["onboarding/PairStep.tsx"], calls: ["listDevices"] },
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
  { screen: "/accounts", feature: "Accounts and models per credential source, usage windows, rate-limit blocks", sources: ["pages/AccountsPage.tsx"], calls: ["accounts"] },
  { screen: "/vault", feature: "Vault status: set up, unlock (passphrase or OS keyring) and lock", sources: ["pages/VaultPage.tsx"], calls: ["vaultStatus", "setupVault", "unlockVault", "lockVault"] },
  { screen: "/vault", feature: "Add a login or an authenticator; delete an item", sources: ["pages/VaultPage.tsx"], calls: ["createVaultItem", "deleteVaultItem"] },
  { screen: "/vault", feature: "Vault items (metadata only, never values)", sources: ["pages/VaultPage.tsx"], calls: ["vaultItems"] },
  { screen: "/vault", feature: "Per-item use policy", sources: ["pages/VaultPage.tsx"], calls: ["updateVaultItem"] },
  { screen: "/vault", feature: "Audit log with item / session / time filters", sources: ["pages/VaultPage.tsx"], calls: ["vaultAudit"] },
  { screen: "/devices", feature: "Paired devices, revoke after confirmation", sources: ["pages/DevicesPage.tsx"], calls: ["listDevices", "revokeDevice"] },
  { screen: "/config", feature: "Every configuration key, rendered from the daemon's schema; local-only keys read-only for paired devices", sources: ["pages/ConfigPage.tsx"], calls: ["settingsConfig", "updateSettingsConfig"] },
  { screen: "/config", feature: "Edit the whole document as JSON", sources: ["pages/ConfigPage.tsx"], calls: ["updateSettingsConfig"] },
  // The browser extension's side panel imports this same client (G-5: one public API for every client).
  { screen: "extension", feature: "Side panel: pending approval cards on reopen; code requests from any conversation as the shared code form; badge and notification", sources: ["../../extension/src/panel.ts", "../../extension/src/codes.ts"], calls: ["listTickets", "resolveTicket"] },
];

/** Public endpoints with no web UI feature, and why. */
export const UNSURFACED: Partial<Record<Endpoint, string>> = {
  "POST /v1/pairing": "Local socket only (`local_socket_required` over TCP): `openside pair` prints the QR code the `/pair` screen consumes.",
  "POST /v1/devices/pair": "Local socket only: token minting for `openside token create`.",
  "GET /v1/remote/status": "Admin diagnostics for the computer running the daemon: `openside remote status` shows which remote transports are up and their public addresses.",
  "POST /v1/otp": "Machine endpoint for an SMS forwarder holding the `otp_ingest` scope.",
  "POST /v1/sessions/{id}/repl": "Machine endpoint (`browse` scope) for sandboxed tools, MCP servers and extensions.",
  "GET /v1/tools": "Machine endpoint: the catalog `openside mcp` and `openside browser tools` build their direct browser tools from.",
  "POST /v1/sessions/{id}/tools/{name}": "Machine endpoint (`browse` scope) for the CLI, MCP servers and scripts driving a session's browser and human-step tools directly; the tickets it opens appear in the web UI as usual.",
  "GET /v1/sessions/{id}": "The session screen rebuilds the session from its event stream.",
  "DELETE /v1/sessions/{id}": "Not offered in the web UI: the conversation list archives instead of deleting permanently; deletion stays an API (admin) action.",
  "POST /v1/sessions/{id}/queue/{mid}/move": "Not offered in the web UI: the queue offers edit, delete and send now; reordering stays an API action.",
  "GET /v1/sessions/{id}/subagents": "The subagent panel is built from `subagent.*` events.",
  "GET /v1/tickets/{id}": "Ticket cards come from `ticket.*` events of the open session.",
  "GET /v1/memory/{id}": "Search hits already carry the full memory.",
  "GET /v1/routines/{id}": "The routine list already carries every field.",
  "GET /v1/pinned-tabs": "Pinned tabs belong to device sync (`openside sync`); the web UI has no pinned-tab screen.",
  "POST /v1/pinned-tabs": "Pinned tabs belong to device sync (`openside sync`); the web UI has no pinned-tab screen.",
  "DELETE /v1/pinned-tabs/{id}": "Pinned tabs belong to device sync (`openside sync`); the web UI has no pinned-tab screen.",
  "GET /v1/policy": "Live policy rules are edited with `openside policy show/set`; the Configuration section covers the stored `[policy]` defaults.",
  "PUT /v1/policy": "Live policy rules are edited with `openside policy show/set`; the Configuration section covers the stored `[policy]` defaults.",
  "POST /v1/policy/test": "Policy dry-run is a CLI diagnostic (`openside policy test`).",
  "GET /v1/sync": "Device sync needs the sync passphrase on the daemon host: `openside sync status/setup/pair/now`.",
  "POST /v1/sync/setup": "Device sync needs the sync passphrase on the daemon host: `openside sync status/setup/pair/now`.",
  "POST /v1/sync/pair": "Device sync needs the sync passphrase on the daemon host: `openside sync status/setup/pair/now`.",
  "POST /v1/sync/run": "Device sync needs the sync passphrase on the daemon host: `openside sync status/setup/pair/now`.",
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
