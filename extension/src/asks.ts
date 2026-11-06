import { MAX_PROMPT_CHARS } from "../../web/src/lassoProtocol";
import { cropCapture, type Upload } from "./capture";
import { panelAskKey, startChat, type PanelAsk } from "./chat";
import type { TabContext } from "./context";
import { openConversation } from "./desktop";
import { loadLassoSettings, shortcutLabel, shortcutPrompt } from "./lasso-settings";
import { validTabId } from "./messages";
import { MIN_SELECTION, type Rect, type Size } from "./lasso-geometry";

/**
 * A lasso or a text selection waiting for the person to pick a shortcut or type a question in the
 * popover (ask.html). Session storage is closed to content scripts, so the page never reads the
 * capture back, and the entry outlives a service-worker restart while the popover is open.
 */
interface PendingAsk { tabId: number; kind: "text" | "region"; context: TabContext; image?: Upload; created: number }
const ASK = "ask:";
const ASK_TTL_MS = 10 * 60_000;
export const MAX_SELECTION_CHARS = 20_000;
/**
 * Session storage holds 10 MB for the whole extension and a region capture is up to a few MB of
 * base64, so pending asks (and the panel hand-offs they turn into) stop well short of it.
 */
const SESSION_BUDGET_BYTES = 6 * 1024 * 1024;
/** Raised when an ask would not fit in session storage; the page tells the person and lets go. */
export const STORAGE_FULL = "storage_full";
/** Raised when the overlay's tab is no longer the one its window shows, so a capture would shoot another page. */
export const NOT_VISIBLE = "not_visible";

export interface AskDetails { tabId: number; kind: PendingAsk["kind"]; shortcuts: { id: string; label: string }[] }

/** Raised when Chrome refuses the page: the side panel's button grants no activeTab, so it needs a site grant. */
export const NO_ACCESS = "no_access";

/** Shows the lasso overlay in `tabId`; the page script is injected first unless it already runs there. */
export async function startLasso(tabId: number): Promise<void> {
  try {
    await chrome.scripting.executeScript({ target: { tabId }, files: ["lasso.js"] });
  } catch (error) {
    throw /cannot access|permission/i.test(String(error)) ? new Error(NO_ACCESS) : error;
  }
  await chrome.tabs.sendMessage(tabId, { type: "lasso.start" }, { frameId: 0 });
}

/** A pending ask, unless it outlived the TTL: then it is dropped, as storeAsk would. */
async function readAsk(token: string): Promise<PendingAsk | undefined> {
  const key = `${ASK}${token}`;
  const ask = (await chrome.storage.session.get(key))[key] as PendingAsk | undefined;
  if (!ask || Date.now() - ask.created < ASK_TTL_MS) return ask;
  await chrome.storage.session.remove(key);
  return undefined;
}

/**
 * Drops a closed tab's pending ask, which may hold a capture of a few MB, and its unread panel
 * hand-off. Queued behind any write in flight, which re-checks the tab, so neither lands afterwards.
 */
export function dropAsksForTab(tabId: number): Promise<void> {
  return queued(async () => {
    const stored = await chrome.storage.session.get(null);
    const keys = Object.entries(stored).filter(([key, value]) => key === panelAskKey(tabId) || (key.startsWith(ASK) && (value as PendingAsk).tabId === tabId)).map(([key]) => key);
    if (keys.length) await chrome.storage.session.remove(keys);
  });
}

/** Raised when the tab closed while its capture or hand-off was being prepared. */
const TAB_CLOSED = "tab_closed";

async function tabOpen(tabId: number): Promise<void> {
  await chrome.tabs.get(tabId).catch(() => { throw new Error(TAB_CLOSED); });
}

const sizeOf = (key: string, value: unknown) => new TextEncoder().encode(key + JSON.stringify(value)).length;

// Large session writes (asks and panel hand-offs) go one at a time: two messages for the same tab
// could otherwise both see no earlier ask, and each budget check would count against a snapshot the
// other write is about to change.
let storing: Promise<unknown> = Promise.resolve();

function queued<T>(write: () => Promise<T>): Promise<T> {
  const next = storing.then(write);
  storing = next.catch(() => undefined);
  return next;
}

/**
 * Writes `key` if everything in session storage, without `replacing`, still fits the budget with it,
 * then drops `replacing`. Written first so a failed write loses nothing; the budget leaves room in
 * Chrome's quota for both copies in between.
 */
async function writeWithin(stored: Record<string, unknown>, key: string, value: unknown, replacing: string[] = []): Promise<void> {
  const kept = Object.entries(stored).filter(([entry]) => entry !== key && !replacing.includes(entry)).reduce((total, [entry, item]) => total + sizeOf(entry, item), 0);
  if (kept + sizeOf(key, value) > SESSION_BUDGET_BYTES) throw new Error(STORAGE_FULL);
  try {
    await chrome.storage.session.set({ [key]: value });
  } catch (error) {
    throw /quota/i.test(String(error)) ? new Error(STORAGE_FULL) : error;
  }
  const leftover = replacing.filter(entry => entry !== key);
  if (leftover.length) await chrome.storage.session.remove(leftover);
}

/** Hands an ask to the tab's side panel within the same budget as pending asks. */
export function storeHandoff(tabId: number, handoff: PanelAsk, replacing: string[] = []): Promise<void> {
  return queued(async () => {
    await tabOpen(tabId);
    await writeWithin(await chrome.storage.session.get(null), panelAskKey(tabId), handoff, replacing);
  });
}

/**
 * One pending ask per tab, and none older than the TTL: a popover left open elsewhere cannot pile
 * up captures. What is left must still fit the session budget, or the ask is refused.
 */
function storeAsk(ask: PendingAsk): Promise<string> {
  return queued(async () => {
    await tabOpen(ask.tabId);
    const stored = await chrome.storage.session.get(null);
    const stale = Object.entries(stored).filter(([key, value]) => key.startsWith(ASK) && ((value as PendingAsk).tabId === ask.tabId || ask.created - (value as PendingAsk).created >= ASK_TTL_MS)).map(([key]) => key);
    if (stale.length) await chrome.storage.session.remove(stale);
    const token = crypto.randomUUID();
    await writeWithin(Object.fromEntries(Object.entries(stored).filter(([key]) => !stale.includes(key))), `${ASK}${token}`, ask);
    return token;
  });
}

function isRect(value: unknown): value is Rect {
  if (typeof value !== "object" || value === null) return false;
  const rect = value as Record<string, unknown>;
  return [rect.x, rect.y, rect.width, rect.height].every(part => typeof part === "number" && Number.isFinite(part)) && (rect.width as number) >= MIN_SELECTION && (rect.height as number) >= MIN_SELECTION;
}

function isSize(value: unknown): value is Size {
  if (typeof value !== "object" || value === null) return false;
  const size = value as Record<string, unknown>;
  return typeof size.width === "number" && typeof size.height === "number" && size.width > 0 && size.height > 0 && Number.isFinite(size.width) && Number.isFinite(size.height);
}

const isToken = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f-]{36}$/.test(value);

/** A message the page script sent from a tab's top frame, with that tab's identity. */
function pageSender(sender: chrome.runtime.MessageSender): (chrome.tabs.Tab & { id: number }) | null {
  if (sender.id !== chrome.runtime.id || sender.frameId !== 0 || !sender.tab || !validTabId(sender.tab.id) || !sender.url || sender.url.startsWith(chrome.runtime.getURL(""))) return null;
  return sender.tab as chrome.tabs.Tab & { id: number };
}

/** A message from one of the extension's own pages (the popover). */
function extensionSender(sender: chrome.runtime.MessageSender): boolean {
  return sender.id === chrome.runtime.id && (sender.url?.startsWith(chrome.runtime.getURL("")) ?? false);
}

/** sender.tab is a snapshot from when the page sent; the person may have switched tabs since. */
async function inFront(tab: chrome.tabs.Tab & { id: number }): Promise<boolean> {
  const [active] = await chrome.tabs.query({ active: true, windowId: tab.windowId });
  return active?.id === tab.id;
}

async function capture(tab: chrome.tabs.Tab & { id: number }, rect: Rect, viewport: Size): Promise<{ token: string }> {
  // captureVisibleTab shoots whatever the window shows, so the overlay's tab must be in front before and after the shot.
  if (!await inFront(tab)) throw new Error(NOT_VISIBLE);
  const shot = await chrome.tabs.captureVisibleTab(tab.windowId, { format: "png" });
  if (!await inFront(tab)) throw new Error(NOT_VISIBLE);
  const image = await cropCapture(shot, rect, viewport);
  return { token: await storeAsk({ tabId: tab.id, kind: "region", context: { url: tab.url ?? "", title: tab.title ?? "", selection: "" }, image, created: Date.now() }) };
}

async function details(token: string): Promise<AskDetails> {
  const ask = await readAsk(token);
  if (!ask) throw new Error("Ask expired");
  const settings = await loadLassoSettings();
  const shortcuts = (ask.kind === "text" ? settings.text : settings.region).shortcuts;
  return { tabId: ask.tabId, kind: ask.kind, shortcuts: shortcuts.map(shortcut => ({ id: shortcut.id, label: shortcutLabel(shortcut) })) };
}

/**
 * Sends the chosen shortcut or typed question with the capture or selection. `panel` says whether
 * the popover opened the side panel: the panel then sends it into the chat it shows; otherwise a
 * new chat starts here and opens in the web app.
 */
async function send(token: string, choice: { shortcut?: string; prompt?: string }, panel: boolean): Promise<void> {
  const ask = await readAsk(token);
  if (!ask) throw new Error("Ask expired");
  const settings = await loadLassoSettings();
  const shortcut = (ask.kind === "text" ? settings.text : settings.region).shortcuts.find(entry => entry.id === choice.shortcut);
  const text = (choice.prompt ?? (shortcut ? shortcutPrompt(shortcut) : "")).trim();
  if (!text) throw new Error("Nothing to ask");
  const attachments = ask.image ? [ask.image] : undefined;
  // The ask stays pending until it is handed on, so a failed send can be retried from the popover, and never longer, so a retry cannot send it twice.
  if (panel) {
    const handoff: PanelAsk = { text, context: ask.context, ...(attachments ? { attachments } : {}) };
    await storeHandoff(ask.tabId, handoff, [`${ASK}${token}`]);
  } else {
    const chat = await startChat(ask.tabId, text, ask.context, attachments);
    await chrome.storage.session.remove(`${ASK}${token}`);
    await openConversation(chat);
  }
  await chrome.tabs.sendMessage(ask.tabId, { type: "lasso.close" }, { frameId: 0 }).catch(() => undefined);
}

/**
 * Drops a pending ask. The page script may only drop its own tab's; when the popover dismissed it,
 * the page's overlay goes too.
 */
async function cancel(token: string, page: (chrome.tabs.Tab & { id: number }) | null): Promise<void> {
  const ask = await readAsk(token);
  if (!ask || (page && ask.tabId !== page.id)) return;
  await chrome.storage.session.remove(`${ASK}${token}`);
  if (!page) await chrome.tabs.sendMessage(ask.tabId, { type: "lasso.close" }, { frameId: 0 }).catch(() => undefined);
}

/**
 * The lasso's messages: `lasso.capture` / `lasso.selection` from the page script, `lasso.details`
 * / `lasso.ask` / `lasso.cancel` from the popover, `lasso.start` from the side panel. Returns null
 * for anything else, so other listeners can answer it.
 */
export function handleLassoMessage(message: unknown, sender: chrome.runtime.MessageSender): Promise<unknown> | null {
  if (typeof message !== "object" || message === null || !("type" in message) || typeof message.type !== "string" || !message.type.startsWith("lasso.")) return null;
  const value = message as Record<string, unknown>;
  const tab = pageSender(sender);
  if (value.type === "lasso.capture" && tab && isRect(value.rect) && isSize(value.viewport)) return capture(tab, value.rect, value.viewport);
  if (value.type === "lasso.selection" && tab && typeof value.selection === "string" && value.selection.trim()) {
    const context = { url: sender.url!, title: tab.title ?? "", selection: value.selection.slice(0, MAX_SELECTION_CHARS) };
    return storeAsk({ tabId: tab.id, kind: "text", context, created: Date.now() }).then(token => ({ token }));
  }
  if (value.type === "lasso.cancel" && (tab || extensionSender(sender)) && isToken(value.token)) return cancel(value.token, tab);
  if (!extensionSender(sender)) return null;
  if (value.type === "lasso.details" && isToken(value.token)) return details(value.token);
  if (value.type === "lasso.ask" && isToken(value.token) && typeof value.panel === "boolean") {
    const prompt = typeof value.prompt === "string" && value.prompt.trim() && [...value.prompt].length <= MAX_PROMPT_CHARS ? value.prompt : undefined;
    const shortcut = typeof value.shortcut === "string" ? value.shortcut : undefined;
    if (!prompt && !shortcut) return null;
    return send(value.token, { ...(prompt ? { prompt } : {}), ...(shortcut ? { shortcut } : {}) }, value.panel);
  }
  if (value.type === "lasso.start" && validTabId(value.tabId)) return startLasso(value.tabId);
  return null;
}
