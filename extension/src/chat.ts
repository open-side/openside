import type { CreateSessionRequest } from "../../web/src/api/types";
import { client } from "./api";
import type { Upload } from "./capture";
import { withContext, type TabContext } from "./context";
import { pairing, rememberSession } from "./storage";

/** What the panel's composer chose before the first message; a lasso or menu ask leaves the daemon's defaults. */
export type ChatOptions = Pick<CreateSessionRequest, "model" | "effort" | "confirmation">;

/** A new chat for `tabId`, remembered so the side panel lists it for that tab, with its first message sent. */
export async function startChat(tabId: number, text: string, context?: TabContext, attachments?: Upload[], options: ChatOptions = {}): Promise<string> {
  const api = client(await pairing());
  const session = await api.createSession({ agent_id: null, title: null, model: null, mode: null, workspace: null, ...options });
  await rememberSession(tabId, session.id);
  await api.submitMessage(session.id, { message_id: crypto.randomUUID(), text: withContext(text, context), mode: "now", ...(attachments?.length ? { attachments } : {}) });
  return session.id;
}

/**
 * What a lasso, a selection shortcut or the "Ask about selection" menu hands the side panel for
 * one tab. An empty `text` attaches the context to the composer for the person to finish; any
 * other text is sent at once into the chat the panel shows (or a new one).
 */
export interface PanelAsk { text: string; context: TabContext; attachments?: Upload[] }
export const panelAskKey = (tabId: number) => `panelAsk:${tabId}`;
