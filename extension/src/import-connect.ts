import { initLocale, localizeDocument, t } from "./i18n";

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const decline = document.querySelector<HTMLButtonElement>("#decline")!;
const question = document.querySelector<HTMLElement>("#question")!;
const status = document.querySelector<HTMLElement>("#status")!;
const warning = document.querySelector<HTMLElement>("#warning")!;
const id = location.hash.slice(1);
const key = `importConnect:${id}`;
let origin = "";

void initLocale(() => { location.reload(); }).then(() => { localizeDocument(); return chrome.storage.session.get(key); }).then(stored => {
  const value: unknown = stored[key];
  if (!/^[0-9a-f-]{36}$/.test(id) || !value || typeof value !== "object" || !("origin" in value) || typeof value.origin !== "string" || !("allowed" in value) || value.allowed !== null) throw new Error("Expired connection request");
  const url = new URL(value.origin);
  if (url.origin !== value.origin || url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname)) throw new Error("Invalid connection origin");
  origin = url.origin;
  question.textContent = t("ext.importConnect.question", { origin });
  if ("changedApp" in value && value.changedApp === true) {
    warning.textContent = t("ext.importConnect.changedApp", { origin });
    warning.hidden = false;
  }
  allow.disabled = false;
  decline.disabled = false;
}).catch(() => { status.textContent = t("ext.importConnect.expired"); });

async function finish(allowed: boolean): Promise<void> {
  await chrome.storage.session.set({ [key]: { origin, allowed } });
  status.textContent = t(allowed ? "ext.importConnect.allowed" : "ext.importConnect.declined");
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
