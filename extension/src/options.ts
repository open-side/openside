import { client } from "./api";
import { parsePairingLink, validateDaemonUrl } from "./storage";
import { clearAgentTabs } from "./agent-tabs";
import { bindStatus, mountIcons } from "./ui";
const status = document.querySelector<HTMLElement>("#status")!;
const notify = bindStatus(status);
mountIcons();
const urlInput = document.querySelector<HTMLInputElement>("#url")!;
const tokenInput = document.querySelector<HTMLInputElement>("#token")!;
const pairingState = document.querySelector<HTMLElement>("#pairing-state")!;
function showPairing(url: string | undefined): void {
  pairingState.textContent = url ? `Paired with ${url}` : "Not paired";
  pairingState.dataset.paired = String(url !== undefined);
}
void chrome.storage.session.get("pairing").then(value => { if (value.pairing) urlInput.value = value.pairing.url; showPairing(value.pairing?.url); });
const linkInput = document.querySelector<HTMLInputElement>("#link")!;
document.querySelector<HTMLFormElement>("#pairing-link")!.addEventListener("submit", event => {
  event.preventDefault();
  let pairing: { url: string; code: string };
  try { pairing = parsePairingLink(linkInput.value); } catch (error) { linkInput.value = ""; notify(error instanceof TypeError ? "That is not a link." : (error as Error).message, "error"); return; }
  const parsed = new URL(pairing.url);
  const hostPermission = `${parsed.protocol}//${parsed.hostname}/*`;
  void chrome.permissions.request({ origins: [hostPermission] }).then(async granted => {
    if (!granted) { notify("Allow daemon access to pair.", "error"); linkInput.value = ""; return; }
    try {
      const device = await client({ url: pairing.url, token: "" }).exchangePairing(pairing.code);
      await chrome.storage.session.set({ pairing: { url: pairing.url, token: device.token } });
      urlInput.value = pairing.url;
      showPairing(pairing.url);
      notify("Paired for this browser session.");
    } catch {
      notify("Pairing failed. The link may have expired or been used; make a new one in openside.", "error");
    } finally {
      linkInput.value = "";
    }
  }).catch(() => { linkInput.value = ""; notify("Daemon access was not granted. Try pairing again.", "error"); });
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
    if (!granted) { notify("Allow daemon access to pair.", "error"); tokenInput.value = ""; return; }
    try {
      await client({ url, token }).listSessions();
      await chrome.storage.session.set({ pairing: { url, token } });
      showPairing(url);
      notify("Paired for this browser session.");
    } catch {
      notify("Pairing failed. Check the daemon URL and device token.", "error");
    } finally {
      tokenInput.value = "";
    }
  }).catch(() => { tokenInput.value = ""; notify("Daemon access was not granted. Try pairing again.", "error"); });
});
document.querySelector("#forget")!.addEventListener("click", () => {
  // A stale optional group must not prevent forgetting credentials.
  void clearAgentTabs().catch(() => undefined).then(() => Promise.all([chrome.storage.session.clear(), chrome.storage.local.remove("importPairing")])).then(() => {
    tokenInput.value = "";
    groupsEnabled = false;
    agentTabs.textContent = "Enable Agent Tabs";
    showPairing(undefined);
    notify("Pairing forgotten.");
  });
});
const agentTabs = document.querySelector<HTMLButtonElement>("#agent-tabs")!;
let groupsEnabled = false;
void chrome.storage.session.get("agentTabsEnabled").then(value => {
  groupsEnabled = value.agentTabsEnabled === true;
  agentTabs.textContent = groupsEnabled ? "Disable Agent Tabs" : "Enable Agent Tabs";
});
agentTabs.addEventListener("click", () => {
  const enabled = !groupsEnabled;
  const permission = enabled ? chrome.permissions.request({ permissions: ["debugger"] }) : Promise.resolve(true);
  void permission.then(async granted => {
    if (!granted) { notify("Agent Tabs permission was not granted.", "error"); return; }
    await chrome.storage.session.set({ agentTabsEnabled: enabled });
    groupsEnabled = enabled;
    agentTabs.textContent = enabled ? "Disable Agent Tabs" : "Enable Agent Tabs";
    notify(enabled ? "Agent Tabs enabled for this browser session." : "Agent Tabs disabled.");
  }).catch(() => { notify("Cannot change Agent Tabs permission.", "error"); });
});
