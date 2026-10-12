import { existsSync, readdirSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";
import { createClient, type Client } from "./client";
import { openSessionStream } from "./sse";
import { manageTarget, WORKSPACE_SECTIONS } from "../pages/manageRoutes";
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
  accounts: (c) => c.accounts(),
  settings: (c) => c.settings(),
  listSessions: (c) => c.listSessions({ q: "x", archived: true }),
  createSession: (c) => c.createSession({}),
  updateSession: (c) => c.updateSession("s1", { title: "t" }),
  stopSession: (c) => c.stopSession("s1"),
  prewarmSession: (c) => c.prewarmSession("s1"),
  prewarmBrowser: (c) => c.prewarmBrowser(),
  submitMessage: (c) => c.submitMessage("s1", { message_id: "m", text: "hi", mode: "queue" }),
  editQueued: (c) => c.editQueued("s1", "m1", "x"),
  deleteQueued: (c) => c.deleteQueued("s1", "m1"),
  promoteQueued: (c) => c.promoteQueued("s1", "m1"),
  resolveTicket: (c) => c.resolveTicket("t1", true),
  rejectTicket: (c) => c.rejectTicket("t1"),
  ticketDone: (c) => c.ticketDone("t1"),
  cancelSubagent: (c) => c.cancelSubagent("s1", "a1"),
  steerSubagent: (c) => c.steerSubagent("s1", "a1", "x"),
  searchSkills: (c) => c.searchSkills("s1", "pdf"),
  listArtifacts: (c) => c.listArtifacts("s1"),
  listVisuals: (c) => c.listVisuals("s1"),
  artifactBlob: (c) => c.artifactBlob("s1", "out/a.txt"),
  takeover: (c) => c.takeover("t1"),
  browserPreview: (c) => c.browserPreview("s1"),
  browserThumbnail: (c) => c.browserThumbnail("s1", "thumb1"),
  browserTabs: (c) => c.browserTabs(),
  listTickets: (c) => c.listTickets("pending"),
  openSessionStream: (c) =>
    new Promise<void>((resolve) => {
      openSessionStream({ client: c, sessionId: "s1", onEvent: () => undefined, onError: () => resolve() });
    }),
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
  searchMemory: (c) => c.searchMemory({ q: "x", kind: "note", limit: 5 }),
  memoryHealth: (c) => c.memoryHealth(),
  createMemory: (c) => c.createMemory({ content: "x", tags: [] }),
  updateMemory: (c) => c.updateMemory("m1", { content: "y" }),
  deleteMemory: (c) => c.deleteMemory("m1"),
  memoryHistory: (c) => c.memoryHistory("m1"),
  settingsConfig: (c) => c.settingsConfig(),
  updateSettingsConfig: (c) => c.updateSettingsConfig({}),
  onboarding: (c) => c.onboarding(),
  setOnboarding: (c) => c.setOnboarding("skipped"),
  externalAgents: (c) => c.externalAgents(),
  installAgentSkill: (c) => c.installAgentSkill("claude", true),
  importProfiles: (c) => c.importProfiles(),
  runImport: (c) => c.runImport({ profile: "chrome:Default", what: null, dry_run: true }),
  vaultStatus: (c) => c.vaultStatus(),
  setupVault: (c) => c.setupVault("passphrase"),
  unlockVault: (c) => c.unlockVault(),
  lockVault: (c) => c.lockVault(),
  createVaultItem: (c) =>
    c.createVaultItem({ id: "site", origins: ["https://site.example"], username: null, password: null, totp: "JBSWY3DPEHPK3PXP", policy: null }),
  deleteVaultItem: (c) => c.deleteVaultItem("site"),
  vaultItems: (c) => c.vaultItems(),
  updateVaultItem: (c) => c.updateVaultItem("site", "never"),
  vaultAudit: (c) => c.vaultAudit({ item: "site", session: "s1", since: "2026-01-01T00:00:00Z", limit: 10 }),
  listDevices: (c) => c.listDevices(),
  revokeDevice: (c) => c.revokeDevice("d1"),
  exchangePairing: (c) => c.exchangePairing("code"),
  vapidKey: (c) => c.vapidKey(),
  listPushSubscriptions: (c) => c.listPushSubscriptions(),
  createPushSubscription: (c) => c.createPushSubscription({ endpoint: "https://push.test/x", keys: { p256dh: "p", auth: "a" } }),
  deletePushSubscription: (c) => c.deletePushSubscription("p1"),
  pushPresence: (c) => c.pushPresence("p1", true),
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
        const needle = call === "openSessionStream" ? "openSessionStream(" : `.${call}(`;
        expect(
          texts.some((text) => text.includes(needle)),
          `${feature.feature}: ${needle} in ${feature.sources.join(", ")}`,
        ).toBe(true);
      }
    }
  });

  it("every app route has features and every feature screen is a route", () => {
    const app = readFileSync(new URL("App.tsx", SRC), "utf8");
    // Settings, Workspace and its sections are dialog routes resolved by `manageTarget`.
    const dialogRoutes = ["/settings", "/workspace", ...WORKSPACE_SECTIONS.map((section) => section.path)];
    expect(dialogRoutes.every((route) => manageTarget(route) !== null)).toBe(true);
    const routes = ["/", ...[...app.matchAll(/<Route path="([^"*]+)"/g)].map((match) => `/${match[1]}`), ...dialogRoutes];
    const screens = new Set(FEATURES.map((feature) => feature.screen));
    expect(routes.filter((route) => !screens.has(route))).toEqual([]);
    // The extension side panel imports this client; it has no route in the web app.
    const outsideRouter = ["token", "shell", "/pair", "extension"];
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
