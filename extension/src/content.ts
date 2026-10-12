import { extractMessage, messageSelector, type Site } from "./extract";
import { messageSite } from "./messages";
declare const __TEST_MATCHES__: boolean;
const site: Site | undefined = messageSite(location.hostname) ?? (__TEST_MATCHES__ && location.hostname === "127.0.0.1" ? (location.pathname.startsWith("/gmail") ? "gmail" : location.pathname.startsWith("/slack") ? "slack" : undefined) : undefined);
if (site) {
  const host = document.createElement("div");
  host.id = "openside-handle-host";
  host.style.cssText = "position:fixed;right:20px;bottom:20px;z-index:2147483647";
  const root = host.attachShadow({ mode: "closed" });
  const button = document.createElement("button");
  button.id = "openside-handle";
  button.textContent = "Handle with openside";
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
    void chrome.runtime.sendMessage({ type: "start", text: "Handle this message.", context: current }).then(result => {
      button.textContent = result.error ? "Pair openside and try again" : "Opened in Ask";
    }, () => { button.textContent = "Cannot reach openside"; }).finally(() => { button.disabled = false; });
  });
  const refresh = () => {
    const available = !!document.querySelector(messageSelector(site));
    if (available && !host.isConnected) document.body.append(host);
    if (!available && host.isConnected) host.remove();
  };
  new MutationObserver(refresh).observe(document.body, { childList: true, subtree: true });
  refresh();
}
