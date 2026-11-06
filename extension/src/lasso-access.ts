import { initLocale, localizeDocument, t } from "./i18n";
import { ALL_SITES } from "./lasso-settings";

const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const status = document.querySelector<HTMLElement>("#status")!;

void initLocale(() => { location.reload(); }).then(async () => {
  localizeDocument();
  const granted = await chrome.permissions.contains(ALL_SITES);
  allow.disabled = granted;
  if (granted) status.textContent = t("ext.lassoAccess.granted");
});

allow.addEventListener("click", () => {
  // Chrome requires this call directly inside the click, before any await. The service worker
  // registers the page script when the permission lands (permissions.onAdded).
  const permission = chrome.permissions.request(ALL_SITES);
  allow.disabled = true;
  status.textContent = t("ext.siteAccess.waiting");
  void permission.then(granted => {
    allow.disabled = granted;
    status.textContent = t(granted ? "ext.lassoAccess.granted" : "ext.lassoAccess.denied");
  }).catch(() => {
    allow.disabled = false;
    status.textContent = t("ext.siteAccess.failed");
  });
});
