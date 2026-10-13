export {};

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const decline = document.querySelector<HTMLButtonElement>("#decline")!;
const question = document.querySelector<HTMLElement>("#question")!;
const status = document.querySelector<HTMLElement>("#status")!;
const warning = document.querySelector<HTMLElement>("#warning")!;
const id = location.hash.slice(1);
const key = `importConnect:${id}`;
let origin = "";

void chrome.storage.session.get(key).then(stored => {
  const value: unknown = stored[key];
  if (!/^[0-9a-f-]{36}$/.test(id) || !value || typeof value !== "object" || !("origin" in value) || typeof value.origin !== "string" || !("allowed" in value) || value.allowed !== null) throw new Error("Expired connection request");
  const url = new URL(value.origin);
  if (url.origin !== value.origin || url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname)) throw new Error("Invalid connection origin");
  origin = url.origin;
  question.textContent = `Connect openside at ${origin} for importing?`;
  if ("changedApp" in value && value.changedApp === true) {
    warning.textContent = `openside at ${origin} could not prove it's the same app you paired before. Connect only if you trust this app.`;
    warning.hidden = false;
  }
  allow.disabled = false;
  decline.disabled = false;
}).catch(() => { status.textContent = "This connection request has expired. Return to openside and choose Connect again."; });

async function finish(allowed: boolean): Promise<void> {
  await chrome.storage.session.set({ [key]: { origin, allowed } });
  status.textContent = allowed ? "Connection allowed. Return to openside; your import will continue there." : "Not connected. Your import is saved; nothing has been copied into this browser.";
}

allow.addEventListener("click", () => {
  if (!origin) return;
  const url = new URL(origin);
  // Optional permissions need the extension's own person click before any asynchronous work.
  const permission = chrome.permissions.request({ origins: [`${url.protocol}//${url.hostname}/*`] });
  allow.disabled = true;
  decline.disabled = true;
  void permission.then(finish).catch(() => finish(false));
});
decline.addEventListener("click", () => {
  if (!origin) return;
  allow.disabled = true;
  decline.disabled = true;
  void finish(false);
});
