import { ledger } from "./api";
import { groupAgentTabs, clearAgentTabs } from "./agent-tabs";
import { dropAsksForTab, handleLassoMessage, MAX_SELECTION_CHARS, startLasso, storeHandoff } from "./asks";
import { startChat } from "./chat";
import { CODE_NOTIFICATION } from "./codes";
import { checkNotifications, handleDesktopRequest, openConversation, SETTLED_NOTIFICATION } from "./desktop";
import { parseOmnibox } from "./context";
import { pairing } from "./storage";
import { parseStart, validTabId } from "./messages";
import { handleImportRequest } from "./imports";
import { IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN, IMPORT_PERMISSION_STATUS } from "../../web/src/import/protocol";
import { initLocale, t } from "./i18n";
import { handleLassoRequest, isLassoRequest, LASSO_COMMAND, loadLassoSettings, syncPageScript } from "./lasso-settings";
import { ADOPT_LAUNCHER_PAIRING, adoptLauncherPairing } from "./launcher";
import { connectPersonBridge, resetPersonBridge } from "./person-bridge";
import { registerShellCommands } from "./shell-commands";
declare const __TEST_MATCHES__: boolean;
void chrome.storage.session.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
// A browser opened by the desktop launcher pairs itself: at startup, and when the launcher opens a
// tab in a browser that is already running but lost its pairing (revoked in Devices).
void adoptLauncherPairing();
// The daemon reads and drives the person's own tabs through this connection; it reconnects on
// every service-worker start and on the alarm, since MV3 workers are stopped when idle.
void connectPersonBridge();
chrome.runtime.onStartup.addListener(() => { void adoptLauncherPairing(); });
chrome.tabs.onCreated.addListener(() => { void adoptLauncherPairing(); });
// The selection dot and the hotkey follow Settings › Lasso and the person's site-access grant, which can change from
// chrome://extensions while no worker runs.
void syncPageScript().catch(() => undefined);
chrome.permissions.onAdded.addListener(() => { void syncPageScript().catch(() => undefined); });
chrome.permissions.onRemoved.addListener(() => { void syncPageScript().catch(() => undefined); });
function open(tabId: number): Promise<void> { return chrome.sidePanel.open({ tabId }); }
chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => undefined);
const ASK_SELECTION_MENU = "ask-selection";
const CAPTURE_MENU = "capture";
// Notifications, pairing replies and the menus read the stored language. The menus outlive the worker,
// so their titles are refreshed whenever the worker starts or the choice changes.
const retitleMenus = () => Promise.all([
  chrome.contextMenus.update(ASK_SELECTION_MENU, { title: t("ext.menu.askSelection") }).catch(() => undefined),
  chrome.contextMenus.update(CAPTURE_MENU, { title: t("ext.menu.capture") }).catch(() => undefined),
]);
const localeReady = initLocale(() => { void retitleMenus(); });
void localeReady.then(retitleMenus).catch(() => undefined);
registerShellCommands(localeReady);
chrome.runtime.onInstalled.addListener(() => {
  void localeReady.catch(() => undefined).then(() => {
    chrome.contextMenus.create({ id: ASK_SELECTION_MENU, title: t("ext.menu.askSelection"), contexts: ["selection"] }, () => void chrome.runtime.lastError);
    // Not on a selection: Chrome folds an extension's items into one submenu when two apply at once, and both stay top level this way.
    chrome.contextMenus.create({ id: CAPTURE_MENU, title: t("ext.menu.capture"), contexts: ["page", "image", "link", "frame", "video"], documentUrlPatterns: ["https://*/*", "http://*/*"] }, () => void chrome.runtime.lastError);
  });
});
chrome.runtime.onMessage.addListener((message: unknown, sender, reply) => {
  const lasso = handleLassoMessage(message, sender);
  if (lasso) {
    void lasso.then(reply, (error: unknown) => reply({ error: error instanceof Error ? error.message : String(error) }));
    return true;
  }
  if (typeof message === "object" && message !== null && "type" in message && message.type === ADOPT_LAUNCHER_PAIRING) {
    if (sender.id !== chrome.runtime.id || !sender.url?.startsWith(chrome.runtime.getURL(""))) return false;
    void adoptLauncherPairing().then(reply, () => reply(false));
    return true;
  }
  const value = parseStart(message, sender, chrome.runtime.getURL(""), __TEST_MATCHES__);
  if (!value) return false;
  const opened = open(value.tabId).catch(() => undefined);
  void startChat(value.tabId, value.text, value.context).then(async id => { await opened; reply({ id }); }, () => reply({ error: "Cannot start a session. Check pairing and token scopes." }));
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
    await startChat(tabId, question);
    await opened;
  }).catch(() => chrome.runtime.openOptionsPage());
});
chrome.contextMenus.onClicked.addListener((info, tab) => {
  // Chrome emits native menu callbacks only for user menu actions; page scripts cannot dispatch them.
  const tabId = tab?.id;
  if (!validTabId(tabId)) return;
  if (info.menuItemId === CAPTURE_MENU) { void startLasso(tabId).catch(() => undefined); return; }
  if (info.menuItemId !== ASK_SELECTION_MENU || !info.selectionText?.trim()) return;
  const context = { url: info.frameUrl ?? info.pageUrl ?? tab?.url ?? "", title: tab?.title ?? "", selection: info.selectionText.slice(0, MAX_SELECTION_CHARS) };
  // The panel only opens inside the click; it then attaches the selection to its composer for the person to finish.
  // Without a panel (or room to hand over) the question cannot wait for words, so a new chat asks the default one and opens in the web app.
  void open(tabId).then(() => storeHandoff(tabId, { text: "", context }))
    .catch(() => startChat(tabId, t("ext.lasso.askSelection.fallback"), context).then(openConversation))
    .catch(() => chrome.runtime.openOptionsPage());
});
chrome.commands.onCommand.addListener((command, tab) => {
  if (command !== LASSO_COMMAND) return;
  // The key press grants activeTab on the tab in front, like the Capture menu item.
  void loadLassoSettings().then(async settings => {
    if (!settings.region.enabled) return;
    const tabId = tab?.id ?? (await chrome.tabs.query({ active: true, currentWindow: true }))[0]?.id;
    if (validTabId(tabId)) await startLasso(tabId);
  }).catch(() => undefined);
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
chrome.alarms.create("person-bridge", { periodInMinutes: 0.5 });
// While a task runs, look again every few seconds: alarms cannot fire faster than every 30 s, which
// would miss tasks that finish in between. Each look is an extension API call, so the worker stays alive.
let watching = false;
async function checkCodes(): Promise<void> {
  const running = await checkNotifications();
  if (running && !watching) {
    watching = true;
    setTimeout(() => { watching = false; void checkCodes().catch(() => undefined); }, 5000);
  }
}
// The web app's Settings toggles talk to the extension here; the sender origin is vetted in desktop.ts, imports.ts and lasso-settings.ts.
chrome.runtime.onMessageExternal.addListener((message: unknown, sender, reply) => {
  if (message && typeof message === "object" && "type" in message && [IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN, IMPORT_PERMISSION_STATUS].includes(String(message.type))) {
    void handleImportRequest(message, sender.origin).then(reply, () => reply({ ok: false, reason: "apply_failed" }));
    return true;
  }
  if (isLassoRequest(message)) {
    void handleLassoRequest(message, sender.origin).then(reply, () => reply({ ok: false, message: t("ext.lasso.settingsFailed") }));
    return true;
  }
  void handleDesktopRequest(message, sender.origin).then(result => { reply(result); if (result.ok && result.enabled) void checkCodes().catch(() => undefined); });
  return true;
});
chrome.alarms.onAlarm.addListener(alarm => {
  if (alarm.name === "agent-tabs") void syncGroups();
  if (alarm.name === "codes") void checkCodes().catch(() => undefined);
  if (alarm.name === "person-bridge") void connectPersonBridge();
});
chrome.notifications.onClicked.addListener(id => {
  if (id.startsWith(SETTLED_NOTIFICATION)) {
    void chrome.notifications.clear(id);
    void openConversation(id.slice(SETTLED_NOTIFICATION.length)).catch(() => undefined);
    return;
  }
  if (!id.startsWith(CODE_NOTIFICATION)) return;
  void chrome.notifications.clear(id);
  // The click is the person's gesture; the panel lists code requests from every conversation.
  void chrome.sidePanel.open({ windowId: chrome.windows.WINDOW_ID_CURRENT }).catch(() => undefined);
});
chrome.tabs.onUpdated.addListener((_tabId, changes) => { if (changes.status === "complete") void syncGroups(); });
chrome.storage.onChanged.addListener((changes, area) => { if (area === "session" && (changes.pairing || changes.agentTabsEnabled)) void syncGroups(); });
chrome.storage.onChanged.addListener((changes, area) => { if (area === "session" && changes.pairing) resetPersonBridge(); });
chrome.tabs.onRemoved.addListener(tabId => {
  void chrome.storage.session.remove(`sessions:${tabId}`);
  void dropAsksForTab(tabId).catch(() => undefined);
});
