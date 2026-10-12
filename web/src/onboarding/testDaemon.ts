import type { Device, ExternalAgent, ImportProfile, ImportReport, OnboardingState, VaultStatus } from "../api/types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";

export const AGENTS: ExternalAgent[] = [
  { id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "~/.claude/skills" },
  { id: "codex", label: "Codex", detected: false, skill_installed: false, skills_dir: "~/.codex/skills" },
  { id: "omp", label: "omp", detected: true, skill_installed: true, skills_dir: "~/.omp/agent/skills" },
];

export const PROFILES: ImportProfile[] = [
  { id: "chrome:Default", browser: "chrome", name: "Work", available: { history: true, bookmarks: true, cookies: true } },
  { id: "firefox:abc.default", browser: "firefox", name: "default", available: { history: true, bookmarks: false, cookies: false } },
];

export function report(dryRun: boolean, patch: Partial<ImportReport> = {}): ImportReport {
  return {
    dry_run: dryRun,
    source: "chrome",
    profile: "default",
    history: { read: 1204, imported: 1200, skipped: 4, rejected: [], note: null },
    bookmarks: { read: 31, imported: 31, skipped: 0, rejected: [], note: null },
    cookies: { read: 80, imported: 77, skipped: 3, rejected: [], note: null },
    ...patch,
  };
}

export interface DaemonState {
  onboarding: OnboardingState;
  agents: ExternalAgent[];
  profiles: ImportProfile[];
  cookiesWritable: boolean;
  vault: VaultStatus["state"];
  devices: Device[];
  /** Overrides a route's answer; return undefined to fall through. */
  override?: (call: Call) => Response | undefined;
}

/** A daemon with the guide's endpoints; `state` is read on every request, so tests mutate it. */
export function onboardingDaemon(patch: Partial<DaemonState> = {}) {
  const state: DaemonState = {
    onboarding: "pending",
    agents: AGENTS.map((agent) => ({ ...agent })),
    profiles: PROFILES,
    cookiesWritable: true,
    vault: "uninitialized",
    devices: [{ device_id: "d1", name: "This laptop", kind: "person_device", scopes: ["admin"], created_at: "2026-10-01T00:00:00Z" }],
    ...patch,
  };
  const fake = fakeClient((call) => {
    const forced = state.override?.(call);
    if (forced) return forced;
    const path = new URL(call.path, "http://daemon.test").pathname;
    const key = `${call.method} ${path}`;
    if (key === "GET /v1/onboarding") return json(200, { state: state.onboarding, updated_at: null });
    if (key === "PUT /v1/onboarding") {
      state.onboarding = (call.body as { state: OnboardingState }).state;
      return json(200, { state: state.onboarding, updated_at: "2026-10-12T00:00:00Z" });
    }
    if (key === "GET /v1/external-agents") return json(200, { agents: state.agents });
    const install = /^POST \/v1\/external-agents\/([^/]+)\/browser-skill$/.exec(key);
    if (install) {
      const agent = state.agents.find((candidate) => candidate.id === install[1]);
      if (!agent) return json(404, { code: "unknown_agent", message: "unknown agent" });
      if (agent.skill_installed && !(call.body as { force?: boolean }).force) return json(409, { code: "exists", message: "exists" });
      agent.skill_installed = true;
      return json(201, { agent: agent.id, skill: "openside-browser", path: `${agent.skills_dir}/openside-browser`, files: 1 });
    }
    if (key === "GET /v1/import/profiles") return json(200, { profiles: state.profiles, cookies_writable: state.cookiesWritable });
    if (key === "POST /v1/import") return json(200, report((call.body as { dry_run: boolean }).dry_run));
    if (key === "GET /v1/vault") return json(200, { state: state.vault, default_policy: "while_unlocked", backend: null });
    if (key === "GET /v1/devices") return json(200, { devices: state.devices });
    return json(404, { code: "not_found", message: "nope" });
  });
  return { ...fake, state };
}
