import { pairing } from "./storage";

/**
 * Keeps a WebSocket to the paired daemon so it can read and drive the person's own browser:
 * their tabs, groups, bookmarks, history, downloads and extensions, and (for tabs the person or
 * the agent attached to a chat) a page outline and simple page actions. The daemon decides what
 * reaches the agent and asks the person first; this side never reports incognito windows.
 */
export const BRIDGE_PROTOCOL = "openside.person-browser";
const RUN_KEY = "personBridgeRun";
const MAX_OUTLINE = 50_000;
const BROWSER_PAGES = [
  "chrome://bookmarks", "chrome://downloads", "chrome://extensions", "chrome://history", "chrome://settings", "chrome://inspect",
  "chrome://settings/clearBrowserData", "chrome://settings/appearance", "chrome://settings/adPrivacy", "chrome://settings/content",
  "chrome://settings/security", "chrome://settings/languages", "chrome://extensions/shortcuts", "chrome://settings/performance",
  "chrome://profile-picker",
];

export class BridgeError extends Error {
  constructor(readonly code: string) { super(code); }
}

type Params = Record<string, unknown>;
interface WireTab { tab_id: number; window_id: number; index: number; title: string; url: string; favicon_url: string | null; active: boolean; pinned: boolean; audible: boolean; muted: boolean; discarded: boolean; group_id: number | null; incognito: boolean }

export function wireTab(tab: chrome.tabs.Tab): WireTab {
  return {
    tab_id: tab.id ?? -1,
    window_id: tab.windowId,
    index: tab.index,
    title: tab.title ?? "",
    url: tab.url ?? tab.pendingUrl ?? "",
    favicon_url: tab.favIconUrl ?? null,
    active: tab.active,
    pinned: tab.pinned,
    audible: tab.audible ?? false,
    muted: tab.mutedInfo?.muted ?? false,
    discarded: tab.discarded,
    group_id: tab.groupId !== undefined && tab.groupId >= 0 ? tab.groupId : null,
    incognito: tab.incognito,
  };
}

async function ownTab(id: unknown): Promise<chrome.tabs.Tab> {
  if (typeof id !== "number" || !Number.isInteger(id)) throw new BridgeError("invalid_request");
  const tab = await chrome.tabs.get(id).catch(() => null);
  if (!tab || tab.incognito) throw new BridgeError("tab_not_found");
  return tab;
}

function ids(value: unknown): number[] {
  if (!Array.isArray(value) || value.length === 0 || !value.every(id => Number.isInteger(id))) throw new BridgeError("invalid_request");
  return value as number[];
}

function webUrl(value: unknown): string {
  if (typeof value !== "string" || value.length > 8 * 1024) throw new BridgeError("invalid_request");
  let url: URL;
  try { url = new URL(value); } catch { throw new BridgeError("invalid_request"); }
  if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) throw new BridgeError("invalid_request");
  return url.href;
}

async function listTabs() {
  const tabs = (await chrome.tabs.query({})).filter(tab => !tab.incognito && tab.id !== undefined);
  const windows = new Set(tabs.map(tab => tab.windowId));
  const focused = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
  const groups = (await chrome.tabGroups.query({})).filter(group => windows.has(group.windowId));
  return {
    focused_window_id: focused && !focused.incognito ? focused.id ?? null : null,
    tabs: tabs.sort((a, b) => a.windowId - b.windowId || a.index - b.index).map(wireTab),
    groups: groups.map(group => ({ group_id: group.id, window_id: group.windowId, title: group.title ?? "", color: group.color, collapsed: group.collapsed })),
  };
}

const result = (tab: chrome.tabs.Tab | undefined | null, groupId: number | null = null) =>
  ({ tab: tab && !tab.incognito ? wireTab(tab) : null, group_id: groupId });

async function tabCommand(params: Params) {
  switch (params.command) {
    case "activate": {
      const tab = await ownTab(params.tab_id);
      await chrome.tabs.update(tab.id!, { active: true });
      await chrome.windows.update(tab.windowId, { focused: true });
      return result(null);
    }
    case "create": {
      let url: string | undefined;
      if (params.url !== undefined && params.url !== null) {
        const page = String(params.url).replace(/\/$/, "");
        url = BROWSER_PAGES.includes(page) ? page : webUrl(params.url);
      }
      if (params.after_tab_id !== undefined && params.after_tab_id !== null) {
        const after = await ownTab(params.after_tab_id);
        return result(await chrome.tabs.create({ url, windowId: after.windowId, index: after.index + 1, openerTabId: after.id }));
      }
      return result(await chrome.tabs.create({ url }));
    }
    case "close": {
      const tabs = await Promise.all(ids(params.tab_ids).map(ownTab));
      await chrome.tabs.remove(tabs.map(tab => tab.id!));
      return result(null);
    }
    case "duplicate": return result(await chrome.tabs.duplicate((await ownTab(params.tab_id)).id!));
    case "reload": await chrome.tabs.reload((await ownTab(params.tab_id)).id!); return result(null);
    case "set_pinned": return result(await chrome.tabs.update((await ownTab(params.tab_id)).id!, { pinned: params.pinned === true }));
    case "set_muted": return result(await chrome.tabs.update((await ownTab(params.tab_id)).id!, { muted: params.muted === true }));
    case "group": {
      const tabs = await Promise.all(ids(params.tab_ids).map(ownTab));
      const groupId = await chrome.tabs.group({ tabIds: tabs.map(tab => tab.id!), ...(typeof params.group_id === "number" ? { groupId: params.group_id } : {}) });
      const update: chrome.tabGroups.UpdateProperties = {};
      if (typeof params.title === "string") update.title = params.title;
      // The daemon validated the color against Chrome's tab group colors.
      if (typeof params.color === "string") update.color = params.color as NonNullable<chrome.tabGroups.UpdateProperties["color"]>;
      if (Object.keys(update).length) await chrome.tabGroups.update(groupId, update);
      return result(null, groupId);
    }
    case "ungroup": {
      const tabs = await Promise.all(ids(params.tab_ids).map(ownTab));
      await chrome.tabs.ungroup(tabs.map(tab => tab.id!));
      return result(null);
    }
    case "move_to_new_window": {
      const tab = await ownTab(params.tab_id);
      await chrome.windows.create({ tabId: tab.id });
      return result(await chrome.tabs.get(tab.id!));
    }
    case "new_incognito_window": await chrome.windows.create({ incognito: true }); return result(null);
    default: throw new BridgeError("invalid_request");
  }
}

const limit = (value: unknown, fallback: number, max: number) =>
  Math.min(max, Math.max(1, Number.isInteger(value) ? value as number : fallback));
const time = (ms: number | undefined) => ms ? new Date(ms).toISOString() : null;

async function recentlyClosed(params: Params) {
  const sessions = await chrome.sessions.getRecentlyClosed({ maxResults: limit(params.limit, 10, 25) });
  const entries = sessions.flatMap(session => {
    if (session.tab && session.tab.sessionId && !session.tab.incognito) {
      return [{ session_id: session.tab.sessionId, kind: "tab", title: session.tab.title ?? "", url: session.tab.url ?? null, favicon_url: session.tab.favIconUrl ?? null, closed_at: time(session.lastModified * 1000), tab_count: 1 }];
    }
    const window = session.window;
    if (window && window.sessionId && !window.incognito) {
      const tabs = window.tabs ?? [];
      return [{ session_id: window.sessionId, kind: "window", title: tabs[0]?.title ?? "", url: null, favicon_url: tabs[0]?.favIconUrl ?? null, closed_at: time(session.lastModified * 1000), tab_count: tabs.length }];
    }
    return [];
  });
  return { entries };
}

async function restore(params: Params) {
  if (typeof params.session_id !== "string") throw new BridgeError("invalid_request");
  const restored = await chrome.sessions.restore(params.session_id).catch(() => { throw new BridgeError("not_found"); });
  return result(restored.tab ?? restored.window?.tabs?.[0] ?? null);
}

async function history(params: Params) {
  const items = await chrome.history.search({ text: typeof params.query === "string" ? params.query : "", maxResults: limit(params.limit, 25, 100), startTime: 0 });
  return { items: items.filter(item => item.url).map(item => ({ url: item.url!, title: item.title ?? "", last_visit_at: time(item.lastVisitTime), visit_count: item.visitCount ?? 0 })) };
}

interface WireBookmark { id: string; parent_id: string | null; title: string; url: string | null; children: WireBookmark[] }
function wireBookmark(node: chrome.bookmarks.BookmarkTreeNode): WireBookmark {
  return { id: node.id, parent_id: node.parentId ?? null, title: node.title, url: node.url ?? null, children: (node.children ?? []).map(wireBookmark) };
}

async function bookmarkTree() {
  const [root] = await chrome.bookmarks.getTree();
  return { roots: (root?.children ?? []).map(wireBookmark) };
}

async function createBookmark(params: Params) {
  const created = await chrome.bookmarks.create({ parentId: typeof params.parent_id === "string" ? params.parent_id : "1", title: typeof params.title === "string" ? params.title : "", url: webUrl(params.url) })
    .catch(() => { throw new BridgeError("not_found"); });
  return wireBookmark(created);
}

async function removeBookmark(params: Params) {
  if (typeof params.id !== "string") throw new BridgeError("invalid_request");
  const [node] = await chrome.bookmarks.get(params.id).catch(() => { throw new BridgeError("not_found"); });
  // A folder that still holds bookmarks is never removed wholesale.
  await chrome.bookmarks.remove(node.id).catch(() => { throw new BridgeError("invalid_request"); });
  return {};
}

async function downloads(params: Params) {
  const items = await chrome.downloads.search({ orderBy: ["-startTime"], limit: limit(params.limit, 10, 50) });
  return { downloads: items.map(item => ({
    download_id: item.id,
    filename: item.filename.split(/[\\/]/).pop() ?? "",
    url: item.finalUrl || item.url,
    state: item.state === "in_progress" ? "in_progress" : item.state === "complete" ? "complete" : "interrupted",
    received_bytes: Math.max(0, item.bytesReceived),
    total_bytes: item.totalBytes > 0 ? item.totalBytes : null,
    started_at: item.startTime || null,
    incognito: item.incognito,
  })) };
}

async function extensions() {
  const all = await chrome.management.getAll();
  return { extensions: all.map(item => ({ id: item.id, name: item.name, version: item.version, enabled: item.enabled, description: item.description ?? "", kind: item.type === "extension" ? "extension" : item.type === "theme" ? "theme" : "app" })) };
}

const BROWSING_DATA: Record<string, keyof chrome.browsingData.DataTypeSet> = {
  history: "history", cookies: "cookies", cache: "cache", downloads: "downloads", passwords: "passwords", form_data: "formData",
};

/** The person's Delete browsing data dialog, for their browser profile. */
async function clearBrowsingData(params: Params) {
  const since = params.since_ms;
  const types = params.types;
  if (typeof since !== "number" || !Number.isSafeInteger(since) || since < 0 || !Array.isArray(types) || !types.length) throw new BridgeError("invalid_request");
  const set: chrome.browsingData.DataTypeSet = {};
  for (const type of types) {
    const key = typeof type === "string" && Object.hasOwn(BROWSING_DATA, type) ? BROWSING_DATA[type] : undefined;
    if (!key) throw new BridgeError("invalid_request");
    set[key] = true;
  }
  await chrome.browsingData.remove({ since }, set);
  return {};
}

/**
 * Whether `url` is openside's own UI, which holds the person's approvals: the paired daemon's
 * origin, or its port under any loopback alias. Without a pairing there is no daemon to compare
 * against, so every page counts as openside and nothing is read, driven or navigated to.
 */
async function daemonPage(url: URL): Promise<boolean> {
  const daemon = await pairing().then(paired => new URL(paired.url), () => null);
  if (!daemon) return true;
  // URL already folds IPv4 shorthands (127.1) and spells IPv4-mapped IPv6 as [::ffff:7f00:1].
  const loopback = (name: string) => {
    const host = name.replace(/\.+$/, "");
    return host === "localhost" || host.endsWith(".localhost") || /^127\./.test(host) || host === "0.0.0.0"
      || host === "[::1]" || host === "[::]" || /^\[::ffff:(7f[0-9a-f]{2}:[0-9a-f]{1,4}|0:0)\]$/.test(host);
  };
  const port = (address: URL) => address.port || (address.protocol === "https:" ? "443" : "80");
  return url.origin === daemon.origin || (loopback(url.hostname) && loopback(daemon.hostname) && port(url) === port(daemon));
}

/** The attached tab, still on the site the daemon's policy judged, and not openside itself. */
async function pageTab(params: Params): Promise<chrome.tabs.Tab> {
  const tab = await ownTab(params.tab_id);
  let page: URL;
  try { page = new URL(tab.url ?? ""); } catch { throw new BridgeError("unsupported_page"); }
  if (!/^https?:$/.test(page.protocol) || await daemonPage(page)) throw new BridgeError("unsupported_page");
  if (page.origin !== params.origin) throw new BridgeError("origin_changed");
  return tab;
}

/**
 * Agents read and act only on sites granted by name, as site-access.html does (`<origin>/*`).
 * A wildcard grant made for another feature (e.g. `<all_urls>` for Lasso) would also let
 * executeScript succeed, so only patterns naming this exact host count; a pattern without a
 * port covers every port, as in Chrome's own matching.
 */
async function siteAllowed(tab: chrome.tabs.Tab): Promise<boolean> {
  const page = new URL(tab.url!);
  const granted = await chrome.permissions.getAll().catch(() => ({ origins: [] as string[] }));
  return (granted.origins ?? []).some(pattern => {
    const named = /^(https?):\/\/([^/*]+)\/\*$/.exec(pattern);
    if (!named || `${named[1]}:` !== page.protocol) return false;
    let host: URL;
    try { host = new URL(`${named[1]}://${named[2]}/`); } catch { return false; }
    return host.hostname === page.hostname && (host.port === "" || host.port === page.port);
  });
}

async function inject<T>(tab: chrome.tabs.Tab, func: (...args: never[]) => T, args: unknown[] = []): Promise<T> {
  if (await siteAllowed(tab)) {
    try {
      const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab.id! }, func: func as () => T, args: args as [] });
      return frame?.result as T;
    } catch (error) {
      if (!/cannot access|permission/i.test(String(error))) throw new BridgeError("failed");
    }
  }
  await askSiteAccess(tab);
  throw new BridgeError("site_access_required");
}

const asked = new Map<string, number>();
const ASK_WINDOW_MS = 60_000;
const ASKS_PER_WINDOW = 3;

/**
 * Site access can only be granted from a click on an extension page, so open one next to the
 * tab: at most once a minute per site, and at most three pages a minute across all sites, so
 * agents touching many sites cannot flood the person's browser with tabs.
 */
async function askSiteAccess(tab: chrome.tabs.Tab): Promise<void> {
  const origin = new URL(tab.url!).origin;
  const now = Date.now();
  for (const [site, at] of asked) if (at <= now - ASK_WINDOW_MS) asked.delete(site);
  if (asked.has(origin) || asked.size >= ASKS_PER_WINDOW) return;
  asked.set(origin, now);
  await chrome.tabs.create({ url: chrome.runtime.getURL(`site-access.html#${encodeURIComponent(origin)}`), windowId: tab.windowId, index: tab.index + 1, openerTabId: tab.id })
    .catch(() => undefined);
}

/** Test hook: forget earlier prompts. */
export function resetSiteAccessPrompts(): void {
  asked.clear();
}

/**
 * Runs in the page's isolated world: an outline of headings, text and controls, each control
 * with a uN ref kept in this world for `actOnPage`. Field values are never read.
 */
export function outlinePage(max: number) {
  const refs = new Map<string, Element>();
  const world = globalThis as unknown as { __opensidePersonRefs: Map<string, Element>; __opensidePersonLabel: (element: Element) => string };
  world.__opensidePersonRefs = refs;
  const lines: string[] = [];
  const out: { ref: string; role: string; name: string }[] = [];
  let size = 0;
  let truncated = false;
  const quote = (text: string) => JSON.stringify(text.replace(/\s+/g, " ").trim().slice(0, 300));
  const push = (line: string) => {
    if (size + line.length > max) { truncated = true; return false; }
    lines.push(line); size += line.length + 1; return true;
  };
  const visible = (element: Element) => {
    const style = getComputedStyle(element);
    return style.display !== "none" && style.visibility !== "hidden" && (element as HTMLElement).offsetParent !== null || element.tagName === "BODY";
  };
  const role = (element: Element): string | null => {
    const explicit = element.getAttribute("role");
    if (explicit && ["button", "link", "checkbox", "radio", "tab", "menuitem", "switch", "option", "combobox", "textbox"].includes(explicit)) return explicit;
    switch (element.tagName) {
      case "A": return element.hasAttribute("href") ? "link" : null;
      case "BUTTON": return "button";
      case "SELECT": return "combobox";
      case "TEXTAREA": return "textbox";
      case "INPUT": {
        const type = (element as HTMLInputElement).type;
        if (type === "hidden") return null;
        if (["button", "submit", "reset"].includes(type)) return "button";
        if (type === "checkbox" || type === "radio") return type;
        return type === "password" ? "password" : "textbox";
      }
    }
    return (element as HTMLElement).isContentEditable && !(element.parentElement as HTMLElement | null)?.isContentEditable ? "textbox" : null;
  };
  // Fields are named by their label only: their text content is what the person typed.
  const label = (element: Element, kind: string) => {
    const input = element as HTMLInputElement;
    const labelled = input.labels?.[0]?.textContent ?? element.getAttribute("aria-label") ?? element.getAttribute("title") ?? input.placeholder;
    const field = kind === "textbox" || kind === "password" || kind === "combobox";
    const text = field || element.tagName === "INPUT" ? "" : element.textContent ?? "";
    return (labelled || text || (["button", "submit"].includes(input.type) ? input.value : "") || "").replace(/\s+/g, " ").trim().slice(0, 200);
  };
  world.__opensidePersonLabel = element => label(element, role(element) ?? "");
  const walk = (element: Element) => {
    if (truncated || ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG"].includes(element.tagName.toUpperCase()) || !visible(element)) return;
    const kind = role(element);
    if (kind) {
      const ref = `u${refs.size + 1}`;
      refs.set(ref, element);
      const name = label(element, kind);
      out.push({ ref, role: kind, name });
      push(`${kind} ${quote(name)} [ref=${ref}]`);
      return;
    }
    if (/^H[1-6]$/.test(element.tagName)) { push(`heading ${quote(element.textContent ?? "")}`); return; }
    for (const node of Array.from(element.childNodes)) {
      if (node.nodeType === Node.TEXT_NODE) {
        const text = (node.textContent ?? "").trim();
        if (text) push(quote(text));
      } else if (node.nodeType === Node.ELEMENT_NODE) walk(node as Element);
    }
  };
  if (document.body) walk(document.body);
  return { url: location.href, title: document.title, text: lines.join("\n"), refs: out, truncated };
}

interface PageAction { action: string; origin?: string; ref?: string; expect_name?: string; text?: string; submit?: boolean; key?: string; direction?: string }

/** Runs in the page's isolated world against the refs of the last `outlinePage`. */
export function actOnPage(request: PageAction): { error?: string } {
  // The tab may have navigated between the extension's check and this injection.
  if (request.origin !== undefined && location.origin !== request.origin) return { error: "origin_changed" };
  const world = globalThis as unknown as { __opensidePersonRefs?: Map<string, Element>; __opensidePersonLabel?: (element: Element) => string };
  const element = request.ref ? world.__opensidePersonRefs?.get(request.ref) : undefined;
  if (request.ref && (!element || !element.isConnected)) return { error: "stale_ref" };
  // The daemon judged the action by the label in the snapshot; a relabelled element needs a new look.
  if (element && request.expect_name !== undefined && world.__opensidePersonLabel?.(element) !== request.expect_name) return { error: "stale_ref" };
  const target = element as HTMLElement | undefined;
  const key = (to: Element, name: string) => {
    for (const type of ["keydown", "keyup"]) to.dispatchEvent(new KeyboardEvent(type, { key: name, bubbles: true }));
  };
  switch (request.action) {
    case "click": target!.scrollIntoView({ block: "center" }); target!.click(); return {};
    case "type": {
      target!.focus();
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        const prototype = target instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
        Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(target, request.text ?? "");
        target.dispatchEvent(new Event("input", { bubbles: true }));
        target.dispatchEvent(new Event("change", { bubbles: true }));
      } else if (target?.isContentEditable) {
        target.textContent = request.text ?? "";
        target.dispatchEvent(new Event("input", { bubbles: true }));
      } else return { error: "invalid_request" };
      if (request.submit) {
        key(target, "Enter");
        (target as HTMLInputElement).form?.requestSubmit();
      }
      return {};
    }
    case "select": {
      if (!(target instanceof HTMLSelectElement)) return { error: "invalid_request" };
      const wanted = (request.text ?? "").trim();
      const option = Array.from(target.options).find(item => item.label.trim() === wanted || item.value === wanted);
      if (!option) return { error: "invalid_request" };
      target.value = option.value;
      target.dispatchEvent(new Event("change", { bubbles: true }));
      return {};
    }
    case "press": {
      const to = target ?? document.activeElement ?? document.body;
      key(to, request.key ?? "");
      if (request.key === "Enter") (to as HTMLInputElement).form?.requestSubmit();
      return {};
    }
    case "scroll":
      if (target) target.scrollIntoView({ block: "center" });
      else window.scrollBy(0, (request.direction === "up" ? -0.8 : 0.8) * window.innerHeight);
      return {};
    default: return { error: "invalid_request" };
  }
}

async function snapshot(params: Params) {
  const tab = await pageTab(params);
  const outline = await inject(tab, outlinePage, [MAX_OUTLINE]);
  if (!outline) throw new BridgeError("failed");
  // A navigation during the injection reads another site: drop it rather than send it.
  if (new URL(outline.url).origin !== params.origin) throw new BridgeError("origin_changed");
  return outline;
}

async function act(params: Params) {
  const tab = await pageTab(params);
  if (params.action === "navigate") {
    const url = webUrl(params.url);
    if (await daemonPage(new URL(url))) throw new BridgeError("unsupported_page");
    await chrome.tabs.update(tab.id!, { url });
    return { url };
  }
  const request: PageAction = {
    action: String(params.action),
    origin: String(params.origin),
    ref: typeof params.ref === "string" ? params.ref : undefined,
    text: typeof params.text === "string" ? params.text : undefined,
    submit: params.submit === true,
    key: typeof params.key === "string" ? params.key : undefined,
    direction: typeof params.direction === "string" ? params.direction : undefined,
    expect_name: typeof params.expect_name === "string" ? params.expect_name : undefined,
  };
  const outcome = await inject(tab, actOnPage, [request]);
  if (outcome?.error) throw new BridgeError(outcome.error);
  return { url: (await chrome.tabs.get(tab.id!)).url ?? tab.url ?? "" };
}

const METHODS: Record<string, (params: Params) => Promise<unknown>> = {
  "tabs.list": listTabs,
  "tabs.command": tabCommand,
  "sessions.recently_closed": recentlyClosed,
  "sessions.restore": restore,
  "history.search": history,
  "bookmarks.tree": bookmarkTree,
  "bookmarks.create": createBookmark,
  "bookmarks.remove": removeBookmark,
  "downloads.list": downloads,
  "browsing_data.clear": clearBrowsingData,
  "extensions.list": extensions,
  "tab.snapshot": snapshot,
  "tab.act": act,
};

/** Answer one daemon request frame; anything that is not a request (pings) gets no answer. */
export async function answer(frame: string): Promise<string | null> {
  let request: { id?: unknown; method?: unknown; params?: unknown };
  try { request = JSON.parse(frame); } catch { return null; }
  if (typeof request.id !== "string" || typeof request.method !== "string") return null;
  const method = METHODS[request.method];
  try {
    if (!method) throw new BridgeError("invalid_request");
    const params = request.params && typeof request.params === "object" ? request.params as Params : {};
    return JSON.stringify({ id: request.id, result: await method(params) ?? {} });
  } catch (error) {
    return JSON.stringify({ id: request.id, error: { code: error instanceof BridgeError ? error.code : "failed" } });
  }
}

async function runId(): Promise<string> {
  const stored = (await chrome.storage.session.get(RUN_KEY))[RUN_KEY];
  if (typeof stored === "string") return stored;
  const run = crypto.randomUUID();
  await chrome.storage.session.set({ [RUN_KEY]: run });
  return run;
}

let socket: WebSocket | null = null;
let connecting = false;
let generation = 0;

/** Open the bridge if paired and not already open; safe to call on every wake-up. */
export async function connectPersonBridge(): Promise<void> {
  if (connecting || (socket && socket.readyState <= WebSocket.OPEN)) return;
  connecting = true;
  const started = generation;
  let superseded = false;
  try {
    const paired = await pairing().catch(() => null);
    if (!paired) return;
    const url = new URL("/v1/browser/person/bridge", paired.url);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const run = await runId();
    // The pairing changed while this attempt read the old one: start over with the new one.
    if (started !== generation) { superseded = true; return; }
    const opened = new WebSocket(url.href, ["openside.v1", `openside.bearer.${paired.token}`]);
    socket = opened;
    opened.onopen = () => opened.send(JSON.stringify({ kind: "hello", protocol: BRIDGE_PROTOCOL, version: 1, run }));
    opened.onmessage = event => {
      void answer(String(event.data)).then(reply => { if (reply && opened.readyState === WebSocket.OPEN) opened.send(reply); });
    };
    opened.onclose = () => {
      if (socket === opened) socket = null;
      setTimeout(() => { void connectPersonBridge(); }, 5_000);
    };
  } finally {
    connecting = false;
    if (superseded) void connectPersonBridge();
  }
}

/** A changed pairing (new daemon or token) replaces the connection. */
export function resetPersonBridge(): void {
  generation += 1;
  const old = socket;
  socket = null;
  old?.close();
  void connectPersonBridge();
}
