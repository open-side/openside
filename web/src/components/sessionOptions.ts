import type { CreateSessionRequest, PolicyMode } from "../api/types";

export type ThinkingEffort = NonNullable<CreateSessionRequest["effort"]>;
export type SessionSpeed = NonNullable<CreateSessionRequest["speed"]>;
export type SessionProfile = NonNullable<CreateSessionRequest["profile"]>;

/**
 * What the person picked in a new task's toolbar. `null` leaves the field out of the request,
 * so the daemon applies its own default; every field is fixed once the session exists.
 */
export interface SessionOptions {
  mode: PolicyMode | null;
  model: string | null;
  effort: ThinkingEffort | null;
  speed: SessionSpeed | null;
  profile: SessionProfile | null;
  incognito: boolean;
}

export const DEFAULT_SESSION_OPTIONS: SessionOptions = { mode: null, model: null, effort: null, speed: null, profile: null, incognito: false };

export const MODE_OPTIONS: readonly { value: PolicyMode; label: string }[] = [
  { value: "read-only", label: "Read only" },
  { value: "guard", label: "Guard" },
  { value: "full", label: "Full access" },
];

export const EFFORT_OPTIONS: readonly { value: ThinkingEffort; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra high" },
  { value: "max", label: "Max" },
];

const MODE_RANK: Record<PolicyMode, number> = { "read-only": 0, guard: 1, full: 2 };

/**
 * Why `mode` cannot be picked for a new task, or `null` when it can. The agent's `mode` is
 * already its ceiling intersected with the chief's and the daemon default, and the daemon clamps
 * anything looser to it. Separately, a paired device (every web client) gets 403
 * local_socket_required for a mode looser than the daemon default (`deviceLimit`).
 */
export function modeBlock(mode: PolicyMode, ceiling: PolicyMode, deviceLimit: PolicyMode | null, agentName: string): string | null {
  if (MODE_RANK[mode] > MODE_RANK[ceiling]) return `More than ${agentName} may have`;
  if (deviceLimit && MODE_RANK[mode] > MODE_RANK[deviceLimit]) return "Only the computer running openside can allow this";
  return null;
}

export const modeLabel = (mode: PolicyMode) => MODE_OPTIONS.find((option) => option.value === mode)?.label ?? mode;
export const effortLabel = (effort: ThinkingEffort) => EFFORT_OPTIONS.find((option) => option.value === effort)?.label ?? effort;

export function hasOverrides(options: SessionOptions): boolean {
  return (
    options.incognito ||
    options.mode !== null ||
    options.model !== null ||
    options.effort !== null ||
    options.speed !== null ||
    options.profile !== null
  );
}

/**
 * The request for a new task with these options. A private session cannot belong to a person
 * (the daemon answers 400 invalid_browser_space), so it is created unowned and lands in History.
 */
export function createSessionRequest(agentId: string, options: SessionOptions): CreateSessionRequest {
  return {
    agent_id: options.incognito ? null : agentId,
    // Named rather than defaulted: incognito cannot be combined with an isolated space.
    ...(options.incognito ? { incognito: true, browser_space: "shared" as const } : {}),
    ...(options.mode !== null ? { mode: options.mode } : {}),
    ...(options.model !== null ? { model: options.model } : {}),
    ...(options.effort !== null ? { effort: options.effort } : {}),
    ...(options.speed !== null ? { speed: options.speed } : {}),
    ...(options.profile !== null ? { profile: options.profile } : {}),
  };
}
