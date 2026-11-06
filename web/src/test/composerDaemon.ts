import type {
  AttachedTab,
  CatalogModel,
  MyDevice,
  McpServer,
  ModelCatalog,
  PersonTab,
  PolicyMode,
  Preferences,
  Project,
  Session,
  SkillSummary,
  SttStatus,
  Transcript,
} from "../api/types";
import { fakeClient, json, type Call } from "./fakeDaemon";

type Answer = Response | Promise<Response> | undefined;

export interface ComposerDaemonOptions {
  /** Answers first; `undefined` falls through to the composer's defaults below. */
  respond?: (call: Call) => Answer;
  /** Fields of the chat every `GET /v1/sessions/{id}` reports (its id is the one asked for). */
  session?: Partial<Session>;
  catalog?: ModelCatalog;
  policyMode?: PolicyMode;
  deviceKind?: MyDevice["kind"];
  preferences?: Partial<Preferences>;
  stt?: Partial<SttStatus>;
  transcript?: Transcript;
  skills?: SkillSummary[];
  servers?: McpServer[];
  attached?: AttachedTab[];
  /** The person's tabs, for the title and URL `POST …/attached-tabs` echoes back. */
  personTabs?: readonly PersonTab[];
  /** Id of the chat `POST /v1/sessions` creates. */
  createdId?: string;
  /** What `GET /v1/projects` lists; `POST /v1/projects` adds to it. */
  projects?: Project[];
  /** The settings document's verdict on this browser; remote (no local authority) unless false. */
  remote?: boolean;
  /** The folder `POST /v1/folders/pick` answers with; null as if the picker was closed. */
  pickedFolder?: string | null;
}

export function catalogModel(id: string, display: string, patch: Partial<CatalogModel> = {}): CatalogModel {
  return {
    id,
    display_name: display,
    label: display,
    kind: "chat",
    available: true,
    reason: "",
    accounts: [],
    in_catalog: true,
    strengths: "",
    tier: "reasoning",
    ...patch,
  };
}

export const COMPOSER_CATALOG: ModelCatalog = {
  cached_for_ms: 0,
  default_model: "openai:gpt-6.1",
  fallback_models: [],
  truncated: false,
  deep_model: null,
  fast_model: null,
  image_model: null,
  effective_image_model: null,
  standard_model: null,
  visual_model: null,
  providers: [
    {
      provider: "openai",
      display_name: "OpenAI",
      pool: { provider: "openai", accounts: [] },
      models: [
        catalogModel("openai:gpt-6.1", "GPT-6.1", { default_effort: "low" }),
        // The provider picks Luna's level, so its chip reads Default.
        catalogModel("openai:gpt-6-luna", "GPT-6 Luna", { default_effort: null }),
        catalogModel("openai:gpt-old", "GPT Old", { available: false, reason: "No key for this provider" }),
        catalogModel("openai:image-2", "Image 2", { kind: "image" }),
      ],
    },
    {
      provider: "anthropic",
      display_name: "Anthropic",
      pool: { provider: "anthropic", accounts: [] },
      models: [catalogModel("anthropic:claude-sonnet-5.5", "Claude Sonnet 5.5", { default_effort: "high" })],
    },
  ],
};

export function sessionFixture(patch: Partial<Session> = {}): Session {
  return {
    id: "s1",
    activity: "",
    agent_id: null,
    archived: false,
    browser_space: "isolated",
    confirmation: "ask_first",
    context: null,
    effort: null,
    incognito: false,
    last_seq: 0,
    mode: "guard",
    model: "openai:gpt-6.1",
    pinned: false,
    profile: null,
    project_id: null,
    queue: [],
    status: "idle",
    title: "",
    unread: false,
    ...patch,
  };
}

const PREFERENCES: Preferences = {
  agent_tab_cleanup_minutes: 30,
  completion_sound: false,
  computer_use: false,
  confirmation: "ask_first",
  follow_up: "queue",
  mcp_server: false,
  notifications: [],
  save_sessions: true,
};

/**
 * A daemon for the composer: every read it makes on mount and on `@`, the chat it creates and
 * changes, attached tabs, skills, apps and speech-to-text. A PATCH is applied to the chat, so the
 * next read reports it as the real daemon would.
 */
export function composerDaemon(options: ComposerDaemonOptions = {}) {
  const sessions = new Map<string, Session>();
  const sessionOf = (id: string) => sessions.get(id) ?? sessionFixture({ ...options.session, id });
  const projects = [...(options.projects ?? [])];
  const daemon = fakeClient((call) => {
    const answered = options.respond?.(call);
    if (answered) return answered;
    const url = new URL(call.path, "http://daemon.test");
    const path = url.pathname;
    const get = call.method === "GET";
    if (get && path === "/v1/settings/models") return json(200, options.catalog ?? COMPOSER_CATALOG);
    if (get && path === "/v1/settings") return json(200, { default_model: null, fallback_models: [], policy_mode: options.policyMode ?? "full" });
    if (get && path === "/v1/devices/me") return json(200, { device_id: "d1", name: "Laptop", kind: options.deviceKind ?? "person_device", scopes: ["read", "steer"], request: null });
    if (get && path === "/v1/settings/preferences") return json(200, { ...PREFERENCES, ...options.preferences });
    if (path === "/v1/stt") {
      if (get) return json(200, { available: true, max_bytes: 1_000_000, max_seconds: 120, reason: null, ...options.stt });
      return json(200, options.transcript ?? { text: "hello there" });
    }
    if (get && path === "/v1/skills") {
      const q = url.searchParams.get("q") ?? "";
      return json(200, { skills: (options.skills ?? []).filter((skill) => skill.name.startsWith(q)), problems: [] });
    }
    if (get && path === "/v1/mcp/servers") return json(200, { servers: options.servers ?? [] });
    if (get && path === "/v1/projects") return json(200, { projects });
    if (get && path === "/v1/settings/config") return json(200, { config: {}, restart_required: false, file_only: [], remote: options.remote ?? true, remote_editable: [], schema: {} });
    if (call.method === "POST" && path === "/v1/folders/pick") return json(200, { path: options.pickedFolder ?? null });
    if (call.method === "POST" && path === "/v1/projects") {
      const body = call.body as { name: string; root?: string; instructions?: string };
      const id = `p${projects.length + 1}`;
      const created: Project = { id, name: body.name, root: body.root ?? `/data/projects/${id}`, scratch: !body.root, needs_confirmation: false, instructions: body.instructions ?? "", created_at: "2026-11-06T00:00:00Z", updated_at: "2026-11-06T00:00:00Z" };
      projects.push(created);
      return json(201, created);
    }
    const folder = /^\/v1\/sessions\/([^/]+)\/folder$/.exec(path);
    if (folder && call.method === "PUT") {
      const id = decodeURIComponent(folder[1] ?? "");
      const { project_id, workspace } = call.body as { project_id: string | null; workspace: string | null };
      const next = { ...sessionOf(id), project_id, workspace: projects.find((project) => project.id === project_id)?.root ?? workspace };
      sessions.set(id, next);
      return json(200, next);
    }
    if (call.method === "POST" && path === "/v1/sessions") {
      const created = sessionFixture({ ...options.session, ...(call.body as Partial<Session>), id: options.createdId ?? "fresh" });
      sessions.set(created.id, created);
      return json(201, created);
    }
    const tab = /^\/v1\/sessions\/([^/]+)\/attached-tabs(?:\/(\d+))?$/.exec(path);
    if (tab) {
      if (get) return json(200, { tabs: options.attached ?? [] });
      if (call.method === "DELETE") return json(204, undefined);
      const id = (call.body as { tab_id: number }).tab_id;
      const known = options.personTabs?.find((candidate) => candidate.tab_id === id);
      return json(201, { tab_id: id, title: known?.title ?? "", url: known?.url ?? "", attached_at: "2026-11-06T00:00:00Z", attached_by: "person" } satisfies AttachedTab);
    }
    if (/^\/v1\/sessions\/[^/]+\/prewarm$/.test(path)) return json(204, undefined);
    if (/^\/v1\/sessions\/[^/]+\/messages$/.test(path)) return json(202, { id: "m", status: "queued" });
    const chat = /^\/v1\/sessions\/([^/]+)$/.exec(path);
    if (chat) {
      const id = decodeURIComponent(chat[1] ?? "");
      if (get) return json(200, sessionOf(id));
      if (call.method === "PATCH") {
        const next = { ...sessionOf(id), ...(call.body as Partial<Session>) };
        sessions.set(id, next);
        return json(200, next);
      }
    }
    return json(404, { code: "not_found", message: `no fake for ${call.method} ${path}` });
  });
  return {
    ...daemon,
    /** Requests the person caused: everything but reads and the prewarm on focus. */
    writes: () => daemon.calls.filter((call) => call.method !== "GET" && !call.path.endsWith("/prewarm")),
  };
}

export { json, type Call };
