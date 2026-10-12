import type { TabContext } from "./context";

export interface StartRequest { type: "start"; text: string; tabId?: number; context?: TabContext }
export function validTabId(value: unknown): value is number { return typeof value === "number" && Number.isInteger(value) && value >= 0; }
function isContext(value: unknown): value is TabContext {
  if (!value || typeof value !== "object" || !("url" in value) || !("title" in value) || !("selection" in value)) return false;
  return typeof value.url === "string" && typeof value.title === "string" && typeof value.selection === "string";
}
export function sameDocument(left: string, right: string): boolean {
  try { const a = new URL(left); const b = new URL(right); return a.origin === b.origin && a.pathname === b.pathname && a.search === b.search; } catch { return false; }
}
export function parseStart(value: unknown, sender: chrome.runtime.MessageSender, extensionOrigin: string, allowTestMatches = false): (StartRequest & { tabId: number }) | null {
  if (sender.id !== chrome.runtime.id || !value || typeof value !== "object" || !("type" in value) || !("text" in value)) return null;
  if (value.type !== "start" || typeof value.text !== "string" || !value.text.trim()) return null;
  const context = "context" in value ? value.context : undefined;
  if (context !== undefined && !isContext(context)) return null;
  const extensionPage = sender.url?.startsWith(extensionOrigin) ?? false;
  if (!extensionPage && (sender.frameId !== 0 || !sender.tab?.url || !context || !sender.url || !sameDocument(context.url, sender.url) || !sameDocument(sender.url, sender.tab.url))) return null;
  const tabId = extensionPage ? ("tabId" in value ? value.tabId : undefined) : sender.tab?.id;
  if (!validTabId(tabId)) return null;
  if (!extensionPage) {
    try {
      const url = new URL(sender.url!);
      const testPage = allowTestMatches && url.protocol === "http:" && url.hostname === "127.0.0.1" && (url.pathname.startsWith("/gmail") || url.pathname.startsWith("/slack"));
      if (!(url.protocol === "https:" && messageSite(url.hostname)) && !testPage) return null;
    } catch { return null; }
  }
  return { type: "start", tabId, text: value.text, context };
}
export function messageSite(hostname: string): "gmail" | "slack" | undefined {
  return hostname === "mail.google.com" ? "gmail" : hostname.endsWith(".slack.com") ? "slack" : undefined;
}
export function validMenu(info: chrome.contextMenus.OnClickData, tab?: chrome.tabs.Tab): tab is chrome.tabs.Tab & { id: number } {
  if (info.menuItemId !== "handle-openside" || !validTabId(tab?.id) || !info.selectionText?.trim() || (info.frameId !== undefined && info.frameId !== 0) || !info.pageUrl || !tab?.url || !sameDocument(info.pageUrl, tab.url)) return false;
  try { const url = new URL(info.pageUrl); return url.protocol === "https:" && messageSite(url.hostname) !== undefined; } catch { return false; }
}
