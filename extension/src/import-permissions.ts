import { initLocale, localizeDocument, t } from "./i18n";

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const status = document.querySelector<HTMLElement>("#status")!;
const sites = document.querySelector<HTMLElement>("#sites")!;
const id = location.hash.slice(1);
const key = `importPermissions:${id}`;
let origins: string[] = [];
let origin = "";

void initLocale(() => { location.reload(); }).then(() => { localizeDocument(); return chrome.storage.session.get(key); }).then(stored => {
  const saved: unknown = stored[key];
  if (!/^[0-9a-f-]{36}$/.test(id) || !saved || typeof saved !== "object" || !("origins" in saved) || !("origin" in saved) || typeof saved.origin !== "string" || !Array.isArray(saved.origins) || !saved.origins.length || !saved.origins.every(value => typeof value === "string")) {
    status.textContent = t("ext.importPermissions.unavailable");
    return;
  }
  origins = saved.origins;
  origin = saved.origin;
  for (const pattern of origins) {
    const item = document.createElement("li");
    item.textContent = new URL(pattern).hostname;
    sites.append(item);
  }
  allow.disabled = false;
  status.textContent = t("ext.importPermissions.count", { count: origins.length });
}).catch(() => { status.textContent = t("ext.importPermissions.unreadable"); });

allow.addEventListener("click", () => {
  if (!origins.length) return;
  // Chrome requires this call directly inside the click, before any storage or network await.
  const permission = chrome.permissions.request({ origins });
  allow.disabled = true;
  status.textContent = t("ext.importPermissions.waiting");
  void permission.then(async granted => {
    await chrome.storage.session.set({ [key]: { origin, origins, denied: !granted } });
    allow.disabled = granted;
    status.textContent = t(granted ? "ext.importPermissions.granted" : "ext.importPermissions.denied");
  }).catch(async () => {
    await chrome.storage.session.set({ [key]: { origin, origins, denied: true } });
    allow.disabled = false;
    status.textContent = t("ext.importPermissions.failed");
  });
});
