import { initLocale, localizeDocument, t } from "./i18n";

// Chrome shows the microphone prompt only in a tab; the grant then covers the side panel too.
const allow = document.querySelector<HTMLButtonElement>("#allow")!;
const status = document.querySelector<HTMLElement>("#status")!;

async function ask(): Promise<void> {
  allow.disabled = true;
  status.textContent = t("ext.micAccess.waiting");
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    // Only the permission was wanted; nothing is recorded here.
    stream.getTracks().forEach(track => track.stop());
    status.textContent = t("ext.micAccess.granted");
  } catch (cause) {
    allow.disabled = false;
    const name = cause instanceof DOMException ? cause.name : "";
    status.textContent = t(name === "NotFoundError" ? "ext.micAccess.noDevice" : "ext.micAccess.denied");
  }
}

void initLocale(() => { location.reload(); }).then(() => {
  localizeDocument();
  void ask();
});
allow.addEventListener("click", () => { void ask(); });
