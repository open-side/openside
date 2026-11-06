import type { AttachedTab } from "../api/types";

/**
 * What Home could not finish after creating a chat: the words and files stay unsent and the tabs
 * that failed to attach stay as chips, so the new chat's composer can show them with the error
 * and the person decides whether to retry, drop them or send anyway.
 */
export interface ComposerHandoff {
  text: string;
  files: File[];
  tabs: Pick<AttachedTab, "tab_id" | "title" | "url">[];
  error: string;
}

// Module memory, not storage: the handoff only has to survive Home's navigation to the chat.
const handoffs = new Map<string, ComposerHandoff>();

export function leaveHandoff(sessionId: string, handoff: ComposerHandoff): void {
  handoffs.set(sessionId, handoff);
}

export function takeHandoff(sessionId: string): ComposerHandoff | undefined {
  const handoff = handoffs.get(sessionId);
  handoffs.delete(sessionId);
  return handoff;
}
