import { client, ledger } from "./api";
import { groupAgentTabs, clearAgentTabs } from "./agent-tabs";
import { announceCodes, CODE_NOTIFICATION } from "./codes";
import { parseOmnibox, withContext, type TabContext } from "./context";
import { pairing, rememberSession } from "./storage";
import { parseStart, validMenu, validTabId } from "./messages";
declare const __TEST_MATCHES__: boolean;
void chrome.storage.session.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
async function start(tabId: number, text: string, context?: TabContext): Promise<string> {
  const api = client(await pairing());
  const session = await api.createSession({ title: (context?.title || text).slice(0, 200), model: null, mode: null, workspace: null });
  await rememberSession(tabId, session.id);
  await api.submitMessage(session.id, { message_id: crypto.randomUUID(), text: withContext(text, context), mode: "now" });
  return session.id;
}
function open(tabId: number): Promise<void> { return chrome.sidePanel.open({ tabId }); }
chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => undefined);
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({ id: "handle-openside", title: "Handle with openside", contexts: ["selection"], documentUrlPatterns: ["https://mail.google.com/*", "https://*.slack.com/*"] });
});
chrome.runtime.onMessage.addListener((message: unknown, sender, reply) => {
  const value = parseStart(message, sender, chrome.runtime.getURL(""), __TEST_MATCHES__);
  if (!value) return false;
  const opened = open(value.tabId).catch(() => undefined);
  void start(value.tabId, value.text, value.context).then(async id => { await opened; reply({ id }); }, () => reply({ error: "Cannot start a session. Check pairing and token scopes." }));
  return true;
});
chrome.omnibox.onInputEntered.addListener((text) => {
  const question = parseOmnibox(text);
  if (!question) return;
  // The omnibox user gesture ends when this listener returns, so sidePanel.open() cannot wait for tabs.query().
  const opened = chrome.sidePanel.open({ windowId: chrome.windows.WINDOW_ID_CURRENT }).catch(() => undefined);
  void chrome.tabs.query({ active: true, currentWindow: true }).then(async tabs => {
    const tabId = tabs[0]?.id;
    if (!validTabId(tabId)) return;
    await start(tabId, question);
    await opened;
  }).catch(() => chrome.runtime.openOptionsPage());
});
chrome.contextMenus.onClicked.addListener((info, tab) => {
  // Chrome emits native menu callbacks only for user menu actions; page scripts cannot dispatch them.
  if (!validMenu(info, tab)) return;
  void open(tab.id).catch(() => undefined);
  void start(tab.id, "Handle this message.", { url: info.pageUrl!, title: tab.title ?? "", selection: info.selectionText! }).catch(() => chrome.runtime.openOptionsPage());
});
let grouping = false;
let groupingQueued = false;
async function syncGroups(): Promise<void> {
  if (grouping) { groupingQueued = true; return; }
  grouping = true;
  try {
    do {
      groupingQueued = false;
      const enabled = (await chrome.storage.session.get("agentTabsEnabled")).agentTabsEnabled === true;
      if (enabled && await chrome.permissions.contains({ permissions: ["debugger"] })) await groupAgentTabs(await ledger(await pairing()));
      else await clearAgentTabs();
    } while (groupingQueued);
  } catch { /* Optional grouping must not stop Ask. */ }
  finally { grouping = false; }
}
chrome.alarms.create("agent-tabs", { periodInMinutes: 0.5 });
chrome.alarms.create("codes", { periodInMinutes: 0.5 });
async function checkCodes(): Promise<void> { await announceCodes((await client(await pairing()).listTickets("pending")).tickets); }
chrome.alarms.onAlarm.addListener(alarm => {
  if (alarm.name === "agent-tabs") void syncGroups();
  if (alarm.name === "codes") void checkCodes().catch(() => undefined);
});
chrome.notifications.onClicked.addListener(id => {
  if (!id.startsWith(CODE_NOTIFICATION)) return;
  void chrome.notifications.clear(id);
  // The click is the person's gesture; the panel lists code requests from every conversation.
  void chrome.sidePanel.open({ windowId: chrome.windows.WINDOW_ID_CURRENT }).catch(() => undefined);
});
chrome.tabs.onUpdated.addListener((_tabId, changes) => { if (changes.status === "complete") void syncGroups(); });
chrome.storage.onChanged.addListener((changes, area) => { if (area === "session" && (changes.pairing || changes.agentTabsEnabled)) void syncGroups(); });
chrome.tabs.onRemoved.addListener(tabId => { void chrome.storage.session.remove(`sessions:${tabId}`); });
