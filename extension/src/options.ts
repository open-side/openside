import { client } from "./api";
import { parsePairingLink, validateDaemonUrl } from "./storage";
import { clearAgentTabs } from "./agent-tabs";
import { bindStatus, mountIcons } from "./ui";
import { initLocale, loadLocalePreference, localizeDocument, saveLocalePreference, t } from "./i18n";
import type { LocalePreference } from "../../web/src/i18n/core";
const status = document.querySelector<HTMLElement>("#status")!;
const notify = bindStatus(status);
mountIcons();
// Choosing a language below also lands here through storage.onChanged and reloads the page in it.
const ready = initLocale(() => { location.reload(); }).then(() => localizeDocument());
const urlInput = document.querySelector<HTMLInputElement>("#url")!;
const tokenInput = document.querySelector<HTMLInputElement>("#token")!;
const pairingState = document.querySelector<HTMLElement>("#pairing-state")!;
function showPairing(url: string | undefined): void {
  pairingState.textContent = url ? t("ext.options.pairedWith", { url }) : t("ext.options.notPaired");
  pairingState.dataset.paired = String(url !== undefined);
}
void ready.then(() => chrome.storage.session.get("pairing")).then(value => { if (value.pairing) urlInput.value = value.pairing.url; showPairing(value.pairing?.url); });
const linkInput = document.querySelector<HTMLInputElement>("#link")!;
document.querySelector<HTMLFormElement>("#pairing-link")!.addEventListener("submit", event => {
  event.preventDefault();
  let pairing: { url: string; code: string };
  try { pairing = parsePairingLink(linkInput.value); } catch (error) { linkInput.value = ""; notify(error instanceof TypeError ? t("ext.options.notALink") : (error as Error).message, "error"); return; }
  const parsed = new URL(pairing.url);
  const hostPermission = `${parsed.protocol}//${parsed.hostname}/*`;
  void chrome.permissions.request({ origins: [hostPermission] }).then(async granted => {
    if (!granted) { notify(t("ext.options.allowAccess"), "error"); linkInput.value = ""; return; }
    try {
      const device = await client({ url: pairing.url, token: "" }).exchangePairing(pairing.code);
      await chrome.storage.session.set({ pairing: { url: pairing.url, token: device.token } });
      urlInput.value = pairing.url;
      showPairing(pairing.url);
      notify(t("ext.options.paired"));
    } catch {
      notify(t("ext.options.linkFailed"), "error");
    } finally {
      linkInput.value = "";
    }
  }).catch(() => { linkInput.value = ""; notify(t("ext.options.accessDenied"), "error"); });
});
document.querySelector<HTMLFormElement>("#pairing")!.addEventListener("submit", event => {
  event.preventDefault();
  let url: string;
  try { url = validateDaemonUrl(urlInput.value); } catch (error) { notify((error as Error).message, "error"); return; }
  const token = tokenInput.value.trim();
  // Request host access while still in the user's gesture, not after a network await.
  const parsed = new URL(url);
  const hostPermission = `${parsed.protocol}//${parsed.hostname}/*`;
  void chrome.permissions.request({ origins: [hostPermission] }).then(async granted => {
    if (!granted) { notify(t("ext.options.allowAccess"), "error"); tokenInput.value = ""; return; }
    try {
      await client({ url, token }).listSessions();
      await chrome.storage.session.set({ pairing: { url, token } });
      showPairing(url);
      notify(t("ext.options.paired"));
    } catch {
      notify(t("ext.options.tokenFailed"), "error");
    } finally {
      tokenInput.value = "";
    }
  }).catch(() => { tokenInput.value = ""; notify(t("ext.options.accessDenied"), "error"); });
});
document.querySelector("#forget")!.addEventListener("click", () => {
  // A stale optional group must not prevent forgetting credentials.
  void clearAgentTabs().catch(() => undefined).then(() => Promise.all([chrome.storage.session.clear(), chrome.storage.local.remove("importPairing")])).then(() => {
    tokenInput.value = "";
    groupsEnabled = false;
    agentTabs.textContent = agentTabsLabel(false);
    showPairing(undefined);
    notify(t("ext.options.forgotten"));
  });
});
const agentTabs = document.querySelector<HTMLButtonElement>("#agent-tabs")!;
let groupsEnabled = false;
const agentTabsLabel = (enabled: boolean) => t(enabled ? "ext.options.agentTabs.disable" : "ext.options.agentTabs.enable");
void ready.then(() => chrome.storage.session.get("agentTabsEnabled")).then(value => {
  groupsEnabled = value.agentTabsEnabled === true;
  agentTabs.textContent = agentTabsLabel(groupsEnabled);
});
agentTabs.addEventListener("click", () => {
  const enabled = !groupsEnabled;
  const permission = enabled ? chrome.permissions.request({ permissions: ["debugger"] }) : Promise.resolve(true);
  void permission.then(async granted => {
    if (!granted) { notify(t("ext.options.agentTabs.denied"), "error"); return; }
    await chrome.storage.session.set({ agentTabsEnabled: enabled });
    groupsEnabled = enabled;
    agentTabs.textContent = agentTabsLabel(enabled);
    notify(t(enabled ? "ext.options.agentTabs.enabled" : "ext.options.agentTabs.disabled"));
  }).catch(() => { notify(t("ext.options.agentTabs.failed"), "error"); });
});
const language = document.querySelector<HTMLSelectElement>("#language")!;
void ready.then(loadLocalePreference).then(preference => { language.value = preference; });
language.addEventListener("change", () => { void saveLocalePreference(language.value as LocalePreference); });
