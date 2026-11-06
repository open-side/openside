import type { ChatConfirmation, CreateSessionRequest, PolicyMode, Session, UpdateSessionRequest } from "../api/types";
import { t, type MessageKey } from "../i18n";

export type ThinkingEffort = NonNullable<CreateSessionRequest["effort"]>;
export type SessionProfile = NonNullable<CreateSessionRequest["profile"]>;

/**
 * A chat's composer options. `null` leaves the field to the daemon (preferences, the model's own
 * effort, the daemon default ceiling). Before the chat exists they go into its create request;
 * afterwards mode, model, effort and confirmation are changed with PATCH, while profile and
 * incognito stay what the chat was created with. `projectId` and `workspace` (at most one set)
 * say where the chat works; an existing chat changes them with PUT …/folder.
 */
export interface SessionOptions {
  mode: PolicyMode | null;
  model: string | null;
  effort: ThinkingEffort | null;
  profile: SessionProfile | null;
  confirmation: ChatConfirmation | null;
  incognito: boolean;
  projectId: string | null;
  /** An absolute folder of the chat's own, outside any project. */
  workspace: string | null;
}

export const DEFAULT_SESSION_OPTIONS: SessionOptions = { mode: null, model: null, effort: null, profile: null, confirmation: null, incognito: false, projectId: null, workspace: null };

export const MODE_OPTIONS: readonly { value: PolicyMode; labelKey: MessageKey }[] = [
  { value: "read-only", labelKey: "thread.mode.readOnly" },
  { value: "guard", labelKey: "thread.mode.guard" },
  { value: "full", labelKey: "thread.mode.full" },
];

/** The composer's effort ladder; the runtime's `off` and `minimal` stay CLI options. */
export const EFFORT_OPTIONS: readonly { value: ThinkingEffort; labelKey: MessageKey }[] = [
  { value: "low", labelKey: "thread.effort.low" },
  { value: "medium", labelKey: "thread.effort.medium" },
  { value: "high", labelKey: "thread.effort.high" },
  { value: "xhigh", labelKey: "thread.effort.xhigh" },
  { value: "max", labelKey: "thread.effort.max" },
];

export const CONFIRMATION_OPTIONS: readonly { value: ChatConfirmation; labelKey: MessageKey }[] = [
  { value: "ask_first", labelKey: "thread.confirmation.askFirst" },
  { value: "skip", labelKey: "thread.confirmation.skip" },
];

const MODE_RANK: Record<PolicyMode, number> = { "read-only": 0, guard: 1, full: 2 };

/**
 * Why `mode` cannot be picked, or `null` when it can. A paired device (every web client) gets 403
 * local_socket_required for a mode looser than the daemon default (`deviceLimit`), at creation and
 * on PATCH alike.
 */
export function modeBlock(mode: PolicyMode, deviceLimit: PolicyMode | null): string | null {
  if (deviceLimit && MODE_RANK[mode] > MODE_RANK[deviceLimit]) return t("thread.mode.deviceLimit");
  return null;
}

export function modeLabel(mode: PolicyMode): string {
  const key = MODE_OPTIONS.find((option) => option.value === mode)?.labelKey;
  return key ? t(key) : mode;
}

export function effortLabel(effort: ThinkingEffort): string {
  const key = EFFORT_OPTIONS.find((option) => option.value === effort)?.labelKey;
  return key ? t(key) : effort;
}

/** Options of an existing chat, as the daemon reports them. */
export function sessionOptionsOf(session: Session): SessionOptions {
  return {
    mode: session.mode,
    model: session.model,
    effort: session.effort ?? null,
    profile: session.profile ?? null,
    confirmation: session.confirmation,
    incognito: session.incognito,
    projectId: session.project_id ?? null,
    workspace: session.workspace ?? null,
  };
}

/**
 * The request for a new chat with these options. Chats belong to no persona (aside-gap-audit D1),
 * so `agent_id` is always null and the chat lands in the sidebar's Chats.
 */
export function createSessionRequest(options: SessionOptions): CreateSessionRequest {
  return {
    agent_id: null,
    // Named rather than defaulted: incognito cannot be combined with an isolated space.
    ...(options.incognito ? { incognito: true, browser_space: "shared" as const } : {}),
    ...(options.mode !== null ? { mode: options.mode } : {}),
    ...(options.model !== null ? { model: options.model } : {}),
    ...(options.effort !== null ? { effort: options.effort } : {}),
    ...(options.profile !== null ? { profile: options.profile } : {}),
    ...(options.confirmation !== null ? { confirmation: options.confirmation } : {}),
    ...(options.projectId !== null ? { project_id: options.projectId } : options.workspace !== null ? { workspace: options.workspace } : {}),
  };
}

/** The PATCH body for a change of one of the options an existing chat can change. */
export function optionsPatch(change: Partial<SessionOptions>): UpdateSessionRequest {
  return {
    ...(change.mode ? { mode: change.mode } : {}),
    ...(change.model ? { model: change.model } : {}),
    ...(change.effort ? { effort: change.effort } : {}),
    ...(change.confirmation ? { confirmation: change.confirmation } : {}),
  };
}
