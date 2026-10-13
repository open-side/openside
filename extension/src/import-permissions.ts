export {};

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const status = document.querySelector<HTMLElement>("#status")!;
const sites = document.querySelector<HTMLElement>("#sites")!;
const id = location.hash.slice(1);
const key = `importPermissions:${id}`;
let origins: string[] = [];
let origin = "";

void chrome.storage.session.get(key).then(stored => {
  const saved: unknown = stored[key];
  if (!/^[0-9a-f-]{36}$/.test(id) || !saved || typeof saved !== "object" || !("origins" in saved) || !("origin" in saved) || typeof saved.origin !== "string" || !Array.isArray(saved.origins) || !saved.origins.length || !saved.origins.every(value => typeof value === "string")) {
    status.textContent = "This permission request is no longer available. Return to openside and try again.";
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
  status.textContent = `${origins.length} imported ${origins.length === 1 ? "site" : "sites"}. Allow only if you want their sign-ins in this browser.`;
}).catch(() => { status.textContent = "Imported sites could not be read. Return to openside and try again."; });

allow.addEventListener("click", () => {
  if (!origins.length) return;
  // Chrome requires this call directly inside the click, before any storage or network await.
  const permission = chrome.permissions.request({ origins });
  allow.disabled = true;
  status.textContent = "Waiting for site access…";
  void permission.then(async granted => {
    await chrome.storage.session.set({ [key]: { origin, origins, denied: !granted } });
    allow.disabled = granted;
    status.textContent = granted
      ? "Site access allowed. Return to openside; your import will finish there."
      : "Site access was not allowed. Your import is saved; no cookies have been copied. You can try Allow again.";
  }).catch(async () => {
    await chrome.storage.session.set({ [key]: { origin, origins, denied: true } });
    allow.disabled = false;
    status.textContent = "Site access could not be requested. Your import is saved; return to openside and retry.";
  });
});
