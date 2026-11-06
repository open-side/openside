import { extractMessage, messageSelector, type Site } from "./extract";
import { messageSite } from "./messages";
import { initLocale, t, type MessageKey } from "./i18n";
declare const __TEST_MATCHES__: boolean;
const site: Site | undefined = messageSite(location.hostname) ?? (__TEST_MATCHES__ && location.hostname === "127.0.0.1" ? (location.pathname.startsWith("/gmail") ? "gmail" : location.pathname.startsWith("/slack") ? "slack" : undefined) : undefined);
if (site) {
  const host = document.createElement("div");
  host.id = "openside-handle-host";
  host.style.cssText = "position:fixed;right:20px;bottom:20px;z-index:2147483647";
  const root = host.attachShadow({ mode: "closed" });
  const button = document.createElement("button");
  button.id = "openside-handle";
  let label: MessageKey = "ext.common.handleWithOpenside";
  const relabel = () => { button.textContent = t(label); };
  relabel();
  // The button shows at once in the browser's language; the stored choice relabels it when read.
  void initLocale(relabel).then(relabel, () => undefined);
  button.style.cssText = "padding:12px;border:1px solid #888;border-radius:8px;background:#fff;color:#111;cursor:pointer";
  root.append(button);
  let context: ReturnType<typeof extractMessage>;
  button.addEventListener("pointerdown", event => { if (event.isTrusted) context = extractMessage(document, site, location.href); });
  button.addEventListener("click", event => {
    if (!event.isTrusted) return;
    const current = context ?? extractMessage(document, site, location.href);
    context = null;
    if (!current) return;
    button.disabled = true;
    void chrome.runtime.sendMessage({ type: "start", text: t("ext.common.handleThisMessage"), context: current }).then(result => {
      label = result.error ? "ext.content.needsPairing" : "ext.content.opened";
    }, () => { label = "ext.content.unreachable"; }).finally(() => { relabel(); button.disabled = false; });
  });
  const refresh = () => {
    const available = !!document.querySelector(messageSelector(site));
    if (available && !host.isConnected) document.body.append(host);
    if (!available && host.isConnected) host.remove();
  };
  new MutationObserver(refresh).observe(document.body, { childList: true, subtree: true });
  refresh();
}
