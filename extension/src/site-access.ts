import { initLocale, localizeDocument, t } from "./i18n";

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const status = document.querySelector<HTMLElement>("#status")!;
const lead = document.querySelector<HTMLElement>("#lead")!;
const hint = document.querySelector<HTMLElement>("#hint")!;

/** The site an attached-tab read or action needed; only plain http(s) origins are accepted. */
function requestedOrigin(): string | null {
  try {
    const url = new URL(decodeURIComponent(location.hash.slice(1)));
    return (url.protocol === "http:" || url.protocol === "https:") && url.origin === url.href.replace(/\/$/, "") ? url.origin : null;
  } catch {
    return null;
  }
}

const origin = requestedOrigin();

void initLocale(() => { location.reload(); }).then(() => {
  localizeDocument();
  if (!origin) {
    status.textContent = t("ext.siteAccess.invalid");
    return;
  }
  const host = new URL(origin).host;
  lead.textContent = t("ext.siteAccess.lead", { host });
  hint.textContent = t("ext.siteAccess.hint", { host });
  allow.disabled = false;
});

allow.addEventListener("click", () => {
  if (!origin) return;
  // Chrome requires this call directly inside the click, before any await.
  const permission = chrome.permissions.request({ origins: [`${origin}/*`] });
  allow.disabled = true;
  status.textContent = t("ext.siteAccess.waiting");
  void permission.then(granted => {
    allow.disabled = granted;
    status.textContent = t(granted ? "ext.siteAccess.granted" : "ext.siteAccess.denied");
  }).catch(() => {
    allow.disabled = false;
    status.textContent = t("ext.siteAccess.failed");
  });
});
