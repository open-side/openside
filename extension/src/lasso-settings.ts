import { DEFAULT_LASSO, isBuiltin, parseLassoSettings, type BuiltinShortcut, type LassoReply, type LassoRequest, type LassoSettings, type LassoShortcut } from "../../web/src/lassoProtocol";
import { trustedDaemonOrigin } from "./daemonOrigin";
import { t, type MessageKey } from "./i18n";

const SETTINGS = "lassoSettings";
/**
 * The selection dot and the double-press hotkey watch every page, and the hotkey's capture needs
 * `<all_urls>` (captureVisibleTab takes nothing narrower without the activeTab gesture), so both
 * wait for the person to grant it from lasso-access.html. The Capture menu item and the command
 * key work without it through activeTab.
 */
export const ALL_SITES: chrome.permissions.Permissions = { origins: ["<all_urls>"] };
const PAGE_SCRIPT = "lasso-page";
/**
 * Mirrors the `<all_urls>` grant for page scripts already running: they cannot query permissions,
 * and unregistering the content script leaves them alive, so this is how a revocation reaches them.
 */
const ACCESS = "lassoAccess";
export const LASSO_COMMAND = "start-lasso";

export async function loadLassoSettings(): Promise<LassoSettings> {
  return parseLassoSettings((await chrome.storage.local.get(SETTINGS))[SETTINGS]) ?? DEFAULT_LASSO;
}

export function hasAllSites(): Promise<boolean> {
  return chrome.permissions.contains(ALL_SITES);
}

/**
 * Registers the page script on every site while a page feature is on and access is granted, removes
 * it otherwise, and tells running page scripts whether access stands.
 */
export async function syncPageScript(): Promise<void> {
  const settings = await loadLassoSettings();
  const access = await hasAllSites();
  await chrome.storage.local.set({ [ACCESS]: access });
  const wanted = (settings.text.enabled || settings.region.enabled) && access;
  const registered = (await chrome.scripting.getRegisteredContentScripts({ ids: [PAGE_SCRIPT] })).length > 0;
  if (wanted && !registered) await chrome.scripting.registerContentScripts([{ id: PAGE_SCRIPT, matches: ["https://*/*", "http://*/*"], js: ["lasso.js"], runAt: "document_idle", persistAcrossSessions: true }]);
  if (!wanted && registered) await chrome.scripting.unregisterContentScripts({ ids: [PAGE_SCRIPT] });
}

const LABEL: Record<BuiltinShortcut, MessageKey> = {
  summarize: "ext.lasso.builtin.summarize.label",
  translate: "ext.lasso.builtin.translate.label",
  "copy-code": "ext.lasso.builtin.copy-code.label",
  "search-image": "ext.lasso.builtin.search-image.label",
};
const PROMPT: Record<BuiltinShortcut, MessageKey> = {
  summarize: "ext.lasso.builtin.summarize.prompt",
  translate: "ext.lasso.builtin.translate.prompt",
  "copy-code": "ext.lasso.builtin.copy-code.prompt",
  "search-image": "ext.lasso.builtin.search-image.prompt",
};

export function shortcutLabel(shortcut: LassoShortcut): string {
  return shortcut.label ?? (isBuiltin(shortcut.id) ? t(LABEL[shortcut.id]) : shortcut.id);
}

export function shortcutPrompt(shortcut: LassoShortcut): string {
  return shortcut.prompt ?? (isBuiltin(shortcut.id) ? t(PROMPT[shortcut.id]) : "");
}

function parseRequest(raw: unknown): LassoRequest | null {
  if (typeof raw !== "object" || raw === null || !("type" in raw)) return null;
  if (raw.type === "openside.lasso.get" || raw.type === "openside.lasso.access") return { type: raw.type };
  if (raw.type !== "openside.lasso.set" || !("settings" in raw)) return null;
  const settings = parseLassoSettings(raw.settings);
  return settings ? { type: raw.type, settings } : null;
}

export function isLassoRequest(raw: unknown): boolean {
  return typeof raw === "object" && raw !== null && "type" in raw && typeof raw.type === "string" && raw.type.startsWith("openside.lasso.");
}

/** Settings › Lasso in the web app; only the daemon UI the manifest names may ask. */
export async function handleLassoRequest(raw: unknown, senderOrigin: string | undefined): Promise<LassoReply> {
  const request = parseRequest(raw);
  if (!trustedDaemonOrigin(senderOrigin) || !request) return { ok: false, message: t("ext.desktop.refused") };
  if (request.type === "openside.lasso.set") {
    await chrome.storage.local.set({ [SETTINGS]: request.settings });
    await syncPageScript();
  }
  if (request.type === "openside.lasso.access") await chrome.tabs.create({ url: chrome.runtime.getURL("lasso-access.html") });
  const command = (await chrome.commands.getAll()).find(entry => entry.name === LASSO_COMMAND)?.shortcut ?? "";
  return { ok: true, settings: await loadLassoSettings(), access: await hasAllSites(), command };
}
