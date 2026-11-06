import { existsSync, readdirSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";
import { createClient, type Client } from "./client";
import { openComputerUseStream, openSessionStream, pollSessionEvents } from "./sse";
import { SETTINGS_PAGES, settingsPath } from "../settings/nav";
import {
  CALLS,
  FEATURES,
  openApiEndpoints,
  renderUiApiMap,
  UNSURFACED,
  type CallName,
  type OpenApiDocument,
} from "./uiFeatures";

// jsdom gives modules an http import.meta.url; vitest still provides the real __dirname.
const WEB = pathToFileURL(`${__dirname}/../../`);
const SRC = new URL("src/", WEB);
const ROOT = new URL("../", WEB);
const openapi = JSON.parse(readFileSync(new URL("docs/specs/openapi.json", ROOT), "utf8")) as OpenApiDocument;
const endpoints = openApiEndpoints(openapi);

/** Map a concrete request to its OpenAPI template; literal segments beat `{param}` ones. */
function template(method: string, path: string): string | undefined {
  const candidates = endpoints.filter((endpoint) => {
    const [verb, pattern = ""] = endpoint.split(" ");
    const regex = new RegExp(`^${pattern.replace(/\{[^}]+\}/g, "[^/]+")}$`);
    return verb === method && regex.test(path);
  });
  const params = (endpoint: string) => (endpoint.match(/\{/g) ?? []).length;
  return candidates.sort((a, b) => params(a) - params(b))[0];
}

/** One real invocation per call; every argument is a placeholder the fake daemon answers with 404. */
const SAMPLES: Record<CallName, (client: Client, record: (method: string, url: string) => void) => unknown> = {
  health: (c) => c.health(),
  settings: (c) => c.settings(),
  modelCatalog: (c) => c.modelCatalog(),
  setModels: (c) => c.setModels({ default_model: "m", fallback_models: [], fast_model: null }),
  addApiKey: (c) => c.addApiKey("p1", { label: "l", key: "k" }),
  replaceApiKey: (c) => c.replaceApiKey("p1", "k1", { key: "k" }),
  deleteApiKey: (c) => c.deleteApiKey("p1", "k1"),
  deleteSubscriptionAccount: (c) => c.deleteSubscriptionAccount("s1", "a1"),
  oauthProviders: (c) => c.oauthProviders(),
  createOAuthAccount: (c) => c.createOAuthAccount("p1", { label: "l", join_pool: false }),
  deleteOAuthAccount: (c) => c.deleteOAuthAccount("p1", "a1"),
  reloginOAuthAccount: (c) => c.reloginOAuthAccount("p1", "a1"),
  oauthTransaction: (c) => c.oauthTransaction("t1"),
  submitOAuthInput: (c) => c.submitOAuthInput("t1", "c"),
  cancelOAuthTransaction: (c) => c.cancelOAuthTransaction("t1"),
  listPools: (c) => c.listPools(),
  setPool: (c) => c.setPool("p1", []),
  listSessions: (c) => c.listSessions({ q: "x", archived: true }),
  listAgents: (c) => c.listAgents(),
  retireAgent: (c) => c.retireAgent("a1", { successor: "chief", pause_routines: false, revert_hire: false }),
  channelAccounts: (c) => c.channelAccounts(),
  addChannelAccount: (c) =>
    c.addChannelAccount("slack", {
      id: "work",
      label: "Work",
      enabled: false,
      agent_id: null,
      options: { allowed_chats: [], allowed_users: [], allowed_workspaces: [], allowed_channels: [], allowed_guilds: [], api_base: null, listen: null, poll_timeout_secs: null, public_key: null },
      credentials: { token: "t", signing_secret: "s" },
    }),
  updateChannelAccount: (c) => c.updateChannelAccount("slack", "work", { enabled: true }),
  deleteChannelAccount: (c) => c.deleteChannelAccount("slack", "work"),
  testChannelAccount: (c) => c.testChannelAccount("slack", "work"),
  session: (c) => c.session("s1"),
  createSession: (c) => c.createSession({ agent_id: null }),
  updateSession: (c) => c.updateSession("s1", { title: "t" }),
  forkSession: (c) => c.forkSession("s1", { from_message: "m1" }),
  deleteSession: (c) => c.deleteSession("s1"),
  setSessionFolder: (c) => c.setSessionFolder("s1", { project_id: "p1", workspace: null }),
  listProjects: (c) => c.listProjects(),
  createProject: (c) => c.createProject({ name: "n" }),
  updateProject: (c) => c.updateProject("p1", { name: "n" }),
  deleteProject: (c) => c.deleteProject("p1"),
  pickFolder: (c) => c.pickFolder(),
  stopSession: (c) => c.stopSession("s1"),
  prewarmSession: (c) => c.prewarmSession("s1"),
  attachedTabs: (c) => c.attachedTabs("s1"),
  attachTab: (c) => c.attachTab("s1", 7),
  detachTab: (c) => c.detachTab("s1", 7),
  submitMessage: (c) => c.submitMessage("s1", { message_id: "m", text: "hi", mode: "queue" }),
  editQueued: (c) => c.editQueued("s1", "m1", "x"),
  deleteQueued: (c) => c.deleteQueued("s1", "m1"),
  promoteQueued: (c) => c.promoteQueued("s1", "m1"),
  resolveTicket: (c) => c.resolveTicket("t1", true),
  rejectTicket: (c) => c.rejectTicket("t1"),
  ticketDone: (c) => c.ticketDone("t1"),
  cancelSubagent: (c) => c.cancelSubagent("s1", "a1"),
  searchSkills: (c) => c.searchSkills("s1", "pdf"),
  mcpServers: (c) => c.mcpServers(),
  createMcpServer: (c) => c.createMcpServer({ label: "x", transport: "stdio", command: ["x"], cwd: null, enabled: null, id: null, secrets: {}, url: null, allow_private_network: false }),
  updateMcpServer: (c) => c.updateMcpServer("m1", { enabled: false }),
  deleteMcpServer: (c) => c.deleteMcpServer("m1"),
  refreshMcpServer: (c) => c.refreshMcpServer("m1"),
  sttStatus: (c) => c.sttStatus(),
  transcribe: (c) => c.transcribe({ audio_base64: "AAAA", mime: "audio/webm" }),
  modelProviders: (c) => c.modelProviders(),
  memoryFiles: (c) => c.memoryFiles(),
  memoryFile: (c) => c.memoryFile("MEMORY.md"),
  updateMemoryFile: (c) => c.updateMemoryFile("MEMORY.md", { markdown: "x", revision: 1 }),
  memoryChanges: (c) => c.memoryChanges({ limit: 50, before: 9 }),
  rebuildMemory: (c) => c.rebuildMemory(),
  revealMemoryFolder: (c) => c.revealMemoryFolder(),
  settingsConfig: (c) => c.settingsConfig(),
  updateSettingsConfig: (c) => c.updateSettingsConfig({}),
  listArtifacts: (c) => c.listArtifacts("s1"),
  listVisuals: (c) => c.listVisuals("s1"),
  artifactBlob: (c) => c.artifactBlob("s1", "out/a.txt"),
  listShares: (c) => c.listShares("s1"),
  createShare: (c) => c.createShare("s1", { artifacts: [], steps: true, expires_in_secs: null }),
  revokeShare: (c) => c.revokeShare("s1", "x1"),
  sharedChat: (c) => c.sharedChat("x1"),
  sharedArtifactBlob: (c) => c.sharedArtifactBlob("x1", 0),
  faviconBlob: (c) => c.faviconBlob("https://shop.example"),
  takeover: (c) => c.takeover("t1"),
  browserPreview: (c) => c.browserPreview("s1"),
  browserControl: (c) => c.browserControl("s1", "tab1"),
  browserThumbnail: (c) => c.browserThumbnail("s1", "thumb1"),
  browserTabs: (c) => c.browserTabs(),
  listTickets: (c) => c.listTickets("pending"),
  openSessionStream: (c) =>
    new Promise<void>((resolve) => {
      openSessionStream({ client: c, sessionId: "s1", onEvent: () => undefined, onError: () => resolve() });
    }),
  pollSessionEvents: (c) => pollSessionEvents(c, "s1", 0, { wait: 1 }),
  sandboxPreview: (c) => c.sandboxPreview("/v1/sessions/s1/artifacts/a.html/preview?exp=1&sig=x"),
  listSkills: (c) => c.listSkills(),
  getSkill: (c) => c.getSkill("pdf"),
  createSkill: (c) => c.createSkill({ name: "pdf", description: "d", body: "", resources: [] }),
  updateSkill: (c) => c.updateSkill("pdf", { enabled: false }),
  deleteSkill: (c) => c.deleteSkill("pdf"),
  listRoutines: (c) => c.listRoutines(),
  createRoutine: (c) =>
    c.createRoutine({ name: "r", schedule: { type: "interval", secs: 60 }, action: { type: "new_session", prompt: "p" } }),
  updateRoutine: (c) => c.updateRoutine("r1", { enabled: false }),
  deleteRoutine: (c) => c.deleteRoutine("r1"),
  runRoutine: (c) => c.runRoutine("r1"),
  routineRuns: (c) => c.routineRuns("r1"),
  routineSuggestions: (c) => c.routineSuggestions(),
  memoryHealth: (c) => c.memoryHealth(),
  onboarding: (c) => c.onboarding(),
  setOnboarding: (c) => c.setOnboarding("skipped"),
  externalAgents: (c) => c.externalAgents(),
  installAgentSkill: (c) => c.installAgentSkill("claude", true),
  importProfiles: (c) => c.importProfiles(),
  runImport: (c) => c.runImport({ profiles: null, what: null }),
  previewImport: (c) => c.previewImport({ profiles: ["chrome:Default"], what: null }),
  importJob: (c) => c.importJob("job1"),
  importPasswordsCsv: (c) => c.importPasswordsCsv("url,username,password\n"),
  listImportDelivery: (c) => c.listImportDelivery(),
  mintImportDeliveryTicket: (c) => c.mintImportDeliveryTicket("batch1", "device1"),
  acknowledgeImportDelivery: (c) => c.acknowledgeImportDelivery("batch1"),
  vaultStatus: (c) => c.vaultStatus(),
  setupVault: (c) => c.setupVault("passphrase"),
  unlockVault: (c) => c.unlockVault(),
  lockVault: (c) => c.lockVault(),
  createVaultItem: (c) =>
    c.createVaultItem({ id: "site", origins: ["https://site.example"], username: null, password: null, totp: "JBSWY3DPEHPK3PXP", policy: null }),
  deleteVaultItem: (c) => c.deleteVaultItem("site"),
  vaultItems: (c) => c.vaultItems(),
  updateVaultItem: (c) => c.updateVaultItem("site", "never"),
  vaultSettings: (c) => c.vaultSettings(),
  updateVaultSettings: (c) =>
    c.updateVaultSettings({ auto_lock: "one_hour", autofill: null, biometric_unlock: null, default_policy: null, disable_in_incognito: null }),
  vaultBiometrics: (c) => c.vaultBiometrics(),
  exportVault: (c) => c.exportVault({ confirm: true, passphrase: null, biometric: false }),
  clearVault: (c) => c.clearVault({ confirm: true, passphrase: null, biometric: false }),
  createVaultRecoveryKey: (c) => c.createVaultRecoveryKey({ confirm: true, passphrase: null, biometric: false }),
  recoverVault: (c) => c.recoverVault({ recovery_key: "AAAA-BBBB", passphrase: null }),
  connections: (c) => c.connections({ agent: "chief", status: "active" }),
  updateConnection: (c) => c.updateConnection("c1", { grants: [{ agent_id: "chief", access: "ask" }] }),
  checkConnection: (c) => c.checkConnection("c1"),
  reloginConnection: (c) => c.reloginConnection("c1"),
  revokeConnection: (c) => c.revokeConnection("c1", { sign_out: true, delete_vault_item: false }),
  createAccountSpace: (c) => c.createAccountSpace("example.com"),
  dismissConnectionCandidate: (c) => c.dismissConnectionCandidate("c1"),
  connectionAudit: (c) => c.connectionAudit({ connection: "c1", agent: "chief", action: "open", limit: 10 }),
  listDevices: (c) => c.listDevices(),
  revokeDevice: (c) => c.revokeDevice("d1"),
  renameDevice: (c) => c.renameDevice("d1", "Phone"),
  myDevice: (c) => c.myDevice(),
  requestAccess: (c) => c.requestAccess(["admin"]),
  accessRequests: (c) => c.accessRequests(),
  approveAccessRequest: (c) => c.approveAccessRequest("r1"),
  denyAccessRequest: (c) => c.denyAccessRequest("r1"),
  exchangePairing: (c) => c.exchangePairing("code"),
  createPairing: (c) => c.createPairing({ name: "My phone", scopes: ["read"], kind: "person_device", via: null }),
  remoteStatus: (c) => c.remoteStatus(),
  setTailscale: (c) => c.setTailscale(true),
  setCloudflare: (c) => c.setCloudflare({ enabled: true, tunnel: "quick", hostname: null, token_file: null }),
  setRelay: (c) => c.setRelay({ enabled: true, url: "https://relay.example.com", token_file: null }),
  disableRemote: (c) => c.disableRemote(),
  enableRemote: (c) => c.enableRemote(),
  vapidKey: (c) => c.vapidKey(),
  listPushSubscriptions: (c) => c.listPushSubscriptions(),
  createPushSubscription: (c) => c.createPushSubscription({ endpoint: "https://push.test/x", keys: { p256dh: "p", auth: "a" } }),
  deletePushSubscription: (c) => c.deletePushSubscription("p1"),
  pushPresence: (c) => c.pushPresence("p1", true),
  setExternalAgent: (c) => c.setExternalAgent("my-agent", "~/skills"),
  deleteExternalAgent: (c) => c.deleteExternalAgent("my-agent"),
  daemonStatus: (c) => c.daemonStatus(),
  restartDaemon: (c) => c.restartDaemon(),
  memoryModel: (c) => c.memoryModel(),
  downloadMemoryModel: (c) => c.downloadMemoryModel(),
  cancelMemoryModel: (c) => c.cancelMemoryModel(),
  policy: (c) => c.policy(),
  setPolicy: (c) => c.setPolicy({ default_mode: "guard", allow_origins: [], deny_origins: [], rules: [] }),
  validatePolicy: (c) => c.validatePolicy({ network_allow: [] }),
  networkPolicy: (c) => c.networkPolicy(),
  setNetworkPolicy: (c) => c.setNetworkPolicy([]),
  syncStatus: (c) => c.syncStatus(),
  setupSync: (c) => c.setupSync({}),
  pairSync: (c) => c.pairSync({ pairing_code: "osync1-x" }),
  unlockSync: (c) => c.unlockSync({}),
  runSync: (c) => c.runSync(),
  setSyncConfig: (c) => c.setSyncConfig({ settings: false, memory_tags: [] }),
  pinnedTabs: (c) => c.pinnedTabs(),
  pinTab: (c) => c.pinTab("https://example.com"),
  unpinTab: (c) => c.unpinTab("t1"),
  about: (c) => c.about(),
  checkForUpdates: (c) => c.checkForUpdates(),
  browserPrefs: (c) => c.browserPrefs(),
  updateBrowserPrefs: (c) => c.updateBrowserPrefs({ spell_check: false }),
  personTabs: (c) => c.personTabs(),
  personTabCommand: (c) => c.personTabCommand({ command: "activate", tab_id: 1 }),
  clearBrowsingData: (c) => c.clearBrowsingData({ types: ["history"], since_ms: 0 }),
  personBookmarks: (c) => c.personBookmarks(),
  createPersonBookmark: (c) => c.createPersonBookmark({ title: "Example", url: "https://example.com" }),
  deletePersonBookmark: (c) => c.deletePersonBookmark("b1"),
  personHistory: (c) => c.personHistory({ q: "example", limit: 10 }),
  personDownloads: (c) => c.personDownloads(10),
  personExtensions: (c) => c.personExtensions(),
  openBrowserTool: (c) => c.openBrowserTool("task_manager"),
  personRecentlyClosed: (c) => c.personRecentlyClosed(5),
  restorePersonRecentlyClosed: (c) => c.restorePersonRecentlyClosed("s1"),
  preferences: (c) => c.preferences(),
  updatePreferences: (c) => c.updatePreferences({ computer_use: true }),
  cliDefaults: (c) => c.cliDefaults(),
  updateCliDefaults: (c) => c.updateCliDefaults({ profile: "isolated" }),
  computerUse: (c) => c.computerUse(),
  stopComputerUse: (c) => c.stopComputerUse("s1"),
  // The first request starts synchronously, so closing right away still records it.
  openComputerUseStream: (c) => openComputerUseStream({ client: c, sessionId: "s1", onEvent: () => undefined }).close(),
};

async function observed(call: CallName): Promise<string[]> {
  const seen: string[] = [];
  const record = (method: string, url: string) => {
    const path = new URL(url, "http://daemon.test").pathname;
    seen.push(template(method, path) ?? `UNKNOWN ${method} ${path}`);
  };
  const client = createClient({
    baseUrl: "http://daemon.test",
    token: () => "token",
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      record(init?.method ?? "GET", String(input));
      return new Response(JSON.stringify({ code: "not_found", message: "not found" }), { status: 404 });
    }) as typeof fetch,
    webSocket: (url) => {
      record("GET", url.replace(/^ws/, "http"));
      return {} as WebSocket;
    },
  });
  try {
    await SAMPLES[call](client, record);
  } catch {
    // The fake daemon answers 404; only the request line matters here.
  }
  return seen;
}

function sourceFiles(dir: URL): URL[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? sourceFiles(new URL(`${entry.name}/`, dir)) : [new URL(entry.name, dir)],
  );
}

const relative = (url: URL) => url.pathname.slice(WEB.pathname.length);

describe("UI ↔ API map (PP-16, E-4, G-5)", () => {
  it("every registered call issues exactly its endpoint, and that endpoint is in openapi.json", async () => {
    for (const call of Object.keys(CALLS) as CallName[]) {
      expect(endpoints, call).toContain(CALLS[call]);
      expect([...new Set(await observed(call))], call).toEqual([CALLS[call]]);
    }
  });

  it("every feature names an endpoint (or why it needs none) and every call has a feature", () => {
    for (const feature of FEATURES) {
      if (feature.calls.length === 0) expect(feature.clientOnly, feature.feature).toBeTruthy();
      else expect(feature.clientOnly, feature.feature).toBeUndefined();
    }
    const used = new Set(FEATURES.flatMap((feature) => feature.calls));
    expect((Object.keys(CALLS) as CallName[]).filter((call) => !used.has(call))).toEqual([]);
  });

  it("each feature's calls appear in its source files", () => {
    for (const feature of FEATURES) {
      const texts = feature.sources.map((source) => {
        const file = new URL(source, SRC);
        expect(existsSync(file), source).toBe(true);
        return readFileSync(file, "utf8");
      });
      for (const call of feature.calls) {
        const needle = call === "openSessionStream" || call === "pollSessionEvents" || call === "openComputerUseStream" ? `${call}(` : `.${call}(`;
        expect(
          texts.some((text) => text.includes(needle)),
          `${feature.feature}: ${needle} in ${feature.sources.join(", ")}`,
        ).toBe(true);
      }
    }
  });

  it("every app route has features and every feature screen is a route", () => {
    const app = readFileSync(new URL("App.tsx", SRC), "utf8");
    // Every Settings page is a route under `settings/*`.
    const settingsRoutes = SETTINGS_PAGES.map(settingsPath);
    const routes = ["/", ...[...app.matchAll(/<Route path="([^"*]+)"/g)].map((match) => `/${match[1]}`), ...settingsRoutes];
    const screens = new Set(FEATURES.map((feature) => feature.screen));
    expect(routes.filter((route) => !screens.has(route))).toEqual([]);
    // The extension side panel imports this client; it has no route in the web app.
    const outsideRouter = ["token", "shell", "/pair", "/share", "extension"];
    expect([...screens].filter((screen) => !routes.includes(screen) && !outsideRouter.includes(screen))).toEqual([]);
  });

  it("every OpenAPI endpoint is used by the UI or has a reason, never both", () => {
    const used = new Set<string>(FEATURES.flatMap((feature) => feature.calls.map((call) => CALLS[call])));
    const unsurfaced = Object.keys(UNSURFACED);
    expect(endpoints.filter((endpoint) => !used.has(endpoint) && !unsurfaced.includes(endpoint))).toEqual([]);
    expect(unsurfaced.filter((endpoint) => used.has(endpoint) || !endpoints.includes(endpoint))).toEqual([]);
    expect(Object.values(UNSURFACED).every((reason) => (reason ?? "").length > 20)).toBe(true);
  });

  it("no web code opens a connection outside src/api", () => {
    // The service worker's own fetches serve the offline shell and never touch /v1 (see sw.js);
    // the relay transport opens the relay WebSocket that carries every API call when paired via a relay.
    const allowed = ["src/api/client.ts", "src/api/sse.ts", "src/api/relay/transport.ts", "public/sw.js"];
    const patterns = [
      /(?<![.\w$])fetch\s*\(/,
      /\b(?:globalThis|window|self)\s*\.\s*fetch\b/,
      /\bXMLHttpRequest\b/,
      /\bEventSource\b/,
      /\bnew\s+WebSocket\b/,
      /\bsendBeacon\b/,
      /\bimportScripts\b/,
    ];
    const files = [...sourceFiles(SRC), ...sourceFiles(new URL("public/", WEB))].filter((file) => {
      const path = relative(file);
      return /\.(ts|tsx|js|mjs)$/.test(path) && !/\.test\.tsx?$/.test(path) && !path.startsWith("src/test/") && path !== "src/api/schema.ts";
    });
    expect(files.length).toBeGreaterThan(20);
    const offenders = files.flatMap((file) => {
      const path = relative(file);
      if (allowed.includes(path)) return [];
      const code = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
      return patterns.filter((pattern) => pattern.test(code)).map((pattern) => `${path}: ${pattern}`);
    });
    expect(offenders).toEqual([]);
  });

  it("docs/specs/ui-api-map.md is current (pnpm -C web gen:ui-map)", () => {
    expect(readFileSync(new URL("docs/specs/ui-api-map.md", ROOT), "utf8")).toBe(renderUiApiMap(openapi));
  });
});
