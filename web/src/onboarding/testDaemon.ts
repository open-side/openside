import type { ImportDeliveryBatch } from "../import/protocol";
import type { AccessRequest, Device, ExternalAgent, ImportBatchReport, ImportJob, ImportProfile, ImportRequest, OnboardingState, RemoteStatus, VaultStatus } from "../api/types";
import { offer } from "../remote/testDaemon";
import { modelRoutes, modelState, type ModelState } from "../models/testDaemon";
import { fakeClient, json, type Call } from "../test/fakeDaemon";

export const AGENTS: ExternalAgent[] = [
  { id: "claude", label: "Claude Code", detected: true, skill_installed: false, skills_dir: "~/.claude/skills", custom: false },
  { id: "codex", label: "Codex", detected: false, skill_installed: false, skills_dir: "~/.codex/skills", custom: false },
  { id: "omp", label: "omp", detected: true, skill_installed: true, skills_dir: "~/.omp/agent/skills", custom: false },
];

const KINDS = ["history", "bookmarks", "cookies", "passwords", "autofill", "tabs", "search_engines", "permissions", "extensions"] as const;

function profile(id: string, browser: ImportProfile["browser"], brand: string, name: string, email: string | null, kinds: readonly string[]): ImportProfile {
  return {
    id,
    browser,
    brand,
    name,
    email,
    avatar: null,
    available: Object.fromEntries(KINDS.map((kind) => [kind, kinds.includes(kind)])) as ImportProfile["available"],
    warnings: [],
  };
}

export const PROFILES: ImportProfile[] = [
  profile("chrome:Default", "chrome", "chrome", "Work", "work@example.test", ["history", "bookmarks", "cookies", "passwords", "tabs"]),
  profile("brave:Default", "chrome", "brave", "Personal", null, ["history", "bookmarks", "passwords"]),
  profile("firefox:abc.default", "firefox", "firefox", "default", null, ["history", "bookmarks"]),
];

/** What each profile above holds, as a preview reports it (and what a run then writes). */
const HOLDINGS: Record<string, Partial<Record<(typeof KINDS)[number], number>>> = {
  "chrome:Default": { history: 1200, bookmarks: 31, cookies: 77, passwords: 12, tabs: 3 },
  "brave:Default": { history: 300, bookmarks: 5, passwords: 4 },
  "firefox:abc.default": { history: 80, bookmarks: 3 },
};

export function batch(dryRun: boolean, request: ImportRequest = { profiles: null, what: null }, vault: { needed: boolean; locked: boolean } = { needed: false, locked: false }): ImportBatchReport {
  const chosen = PROFILES.filter((candidate) => !request.profiles || request.profiles.includes(candidate.id));
  const kinds: readonly string[] = request.what ?? KINDS;
  const totals = Object.fromEntries(KINDS.map((kind) => [kind, 0])) as ImportBatchReport["totals"];
  const profiles = chosen.map((candidate) => {
    const entry: Record<string, unknown> = { id: candidate.id, source: candidate.browser, profile: "default", dry_run: dryRun };
    for (const kind of KINDS) {
      const count = kinds.includes(kind) ? (HOLDINGS[candidate.id]?.[kind] ?? 0) : null;
      entry[kind] = count === null ? null : { read: count, imported: count, skipped: 0, rejected: [], note: null };
      totals[kind] = (totals[kind] ?? 0) + (count ?? 0);
    }
    return entry as ImportBatchReport["profiles"][number];
  });
  return { dry_run: dryRun, profile: "default", profiles, totals, warnings: [], restore_tabs: [], extensions: [], search_engines: [], permissions: [], vault_needed: vault.needed, vault_locked: vault.locked };
}

export function job(patch: Partial<ImportJob> = {}): ImportJob {
  return { id: "job1", state: "running", completed_profiles: 0, total_profiles: 3, current_profile: null, summary: null, error: null, ...patch };
}

export interface DaemonState {
  onboarding: OnboardingState;
  agents: ExternalAgent[];
  profiles: ImportProfile[];
  vault: VaultStatus["state"];
  vaultBackend: VaultStatus["backend"];
  /** Answers to successive `GET /v1/import/jobs/{id}`; the last one repeats. */
  jobPolls: ImportJob[];
  /** Batches waiting for the extension to apply them in this browser. */
  delivery: ImportDeliveryBatch[];
  /** This device's own scopes and its open access request. */
  myScopes: Device["scopes"];
  myRequest: AccessRequest | null;
  devices: Device[];
  models: ModelState;
  /** Remote-access status; `null` answers 403 like a device that may not read it. */
  remote: RemoteStatus | null;
  /** Overrides a route's answer; return undefined to fall through. */
  override?: (call: Call) => Response | undefined;
}

/** A daemon with the guide's endpoints; `state` is read on every request, so tests mutate it. */
export function onboardingDaemon(patch: Partial<DaemonState> = {}) {
  const state: DaemonState = {
    onboarding: "pending",
    agents: AGENTS.map((agent) => ({ ...agent })),
    profiles: PROFILES,
    vault: "uninitialized",
    vaultBackend: "secret_service",
    jobPolls: [job({ state: "completed", completed_profiles: 3, summary: batch(false) })],
    delivery: [],
    myScopes: ["read", "approve"],
    myRequest: null,
    devices: [{ device_id: "d1", name: "This laptop", kind: "person_device", scopes: ["admin"], created_at: "2026-10-01T00:00:00Z" }],
    models: modelState(),
    remote: null,
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
    const vaultFlags = { needed: state.vault === "uninitialized", locked: state.vault === "locked" };
    if (key === "GET /v1/import/delivery") return json(200, { batches: state.delivery });
    if (key.startsWith("POST /v1/import/delivery/")) {
      state.delivery = state.delivery.filter((batch) => !key.includes(`/${batch.id}/ack`));
      return json(204, undefined);
    }
    if (key === "GET /v1/import/profiles") return json(200, { profiles: state.profiles, cookies_writable: true, vault_needed: vaultFlags.needed, vault_locked: vaultFlags.locked, warnings: [] });
    if (key === "POST /v1/import/preview") return json(200, batch(true, call.body as ImportRequest, vaultFlags));
    if (key === "POST /v1/import") {
      const request = call.body as ImportRequest;
      const passwords = (request.what ?? KINDS).includes("passwords") && (batch(true, request).totals.passwords ?? 0) > 0;
      if (passwords && state.vault === "uninitialized") return json(409, { code: "vault_setup_required", message: "vault setup required" });
      if (passwords && state.vault === "locked") return json(409, { code: "vault_locked", message: "vault locked" });
      return json(202, job({ total_profiles: batch(true, request).profiles.length }));
    }
    if (key === "GET /v1/import/jobs/job1") return json(200, state.jobPolls.length > 1 ? state.jobPolls.shift()! : state.jobPolls[0]!);
    if (key === "POST /v1/import/passwords") {
      if (state.vault !== "unlocked") return json(409, { code: state.vault === "locked" ? "vault_locked" : "vault_setup_required", message: "vault" });
      return json(200, { read: 3, imported: 3, skipped: 0, rejected: [], note: null });
    }
    if (key === "GET /v1/vault") return json(200, { state: state.vault, default_policy: "while_unlocked", backend: state.vaultBackend });
    if (key === "POST /v1/vault/setup" || key === "POST /v1/vault/unlock") {
      state.vault = "unlocked";
      return json(200, { state: state.vault, default_policy: "while_unlocked", backend: state.vaultBackend });
    }
    if (key === "GET /v1/devices/me") return json(200, { device_id: "d1", name: "This device", kind: "person_device", scopes: state.myScopes, request: state.myRequest });
    if (key === "POST /v1/devices/me/access-requests") {
      state.myRequest = { id: "r1", device_id: "d1", name: "This device", scopes: (call.body as { scopes: AccessRequest["scopes"] }).scopes, created_at: "2026-10-13T00:00:00Z", expires_at: "2026-10-13T00:10:00Z" };
      return json(201, state.myRequest);
    }
    if (key === "GET /v1/remote/status") {
      return state.remote ? json(200, state.remote) : json(403, { code: "forbidden", message: "forbidden" });
    }
    if (key === "POST /v1/pairing") return json(200, offer());
    if (key === "GET /v1/devices") return json(200, { devices: state.devices });
    const models = modelRoutes(state.models)(call);
    if (models) return models;
    return json(404, { code: "not_found", message: "nope" });
  });
  return { ...fake, state };
}
